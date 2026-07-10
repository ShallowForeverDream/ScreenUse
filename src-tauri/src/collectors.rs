use crate::db::{now, AppDb};
use crate::models::{AnalysisJob, InputStats, MediaChunk, RawActivityEvent, TimeRange};
use anyhow::{anyhow, Result};
use parking_lot::Mutex;
use serde_json::json;
use std::fs::{self, File};
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::time::{sleep, Duration, Instant};
use uuid::Uuid;

pub trait CollectorAdapter {
    fn start(&self, db: Arc<AppDb>) -> Result<()>;
    fn stop(&self) -> Result<()>;
    fn health(&self) -> CollectorHealth;
    fn emit(&self, db: &AppDb, event: RawActivityEvent) -> Result<()>;
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectorHealth {
    pub running: bool,
    pub last_event_at: Option<String>,
    pub last_error: Option<String>,
}

pub struct DesktopCollector {
    running: AtomicBool,
    last_event_at: Mutex<Option<String>>,
    last_error: Mutex<Option<String>>,
}

impl DesktopCollector {
    pub fn new() -> Self {
        Self { running: AtomicBool::new(false), last_event_at: Mutex::new(None), last_error: Mutex::new(None) }
    }

    fn set_error(&self, err: impl ToString) { *self.last_error.lock() = Some(err.to_string()); }
}

impl CollectorAdapter for Arc<DesktopCollector> {
    fn start(&self, db: Arc<AppDb>) -> Result<()> {
        if self.running.swap(true, Ordering::SeqCst) { return Ok(()); }
        let collector = self.clone();
        let db_for_events = db.clone();
        tauri::async_runtime::spawn(async move {
            while collector.running.load(Ordering::SeqCst) {
                match capture_foreground_event() {
                    Ok(event) => {
                        *collector.last_event_at.lock() = Some(event.timestamp.clone());
                        if let Err(err) = collector.emit(&db_for_events, event) { collector.set_error(err); }
                    }
                    Err(err) => collector.set_error(err),
                }
                sleep(Duration::from_secs(10)).await;
            }
        });

        let collector_for_media = self.clone();
        let db_for_media = db.clone();
        tauri::async_runtime::spawn(async move {
            while collector_for_media.running.load(Ordering::SeqCst) {
                if let Err(err) = create_media_chunk(&collector_for_media, &db_for_media).await { collector_for_media.set_error(err); }
            }
        });
        Ok(())
    }

    fn stop(&self) -> Result<()> {
        self.running.store(false, Ordering::SeqCst);
        Ok(())
    }

    fn health(&self) -> CollectorHealth {
        CollectorHealth { running: self.running.load(Ordering::SeqCst), last_event_at: self.last_event_at.lock().clone(), last_error: self.last_error.lock().clone() }
    }

    fn emit(&self, db: &AppDb, event: RawActivityEvent) -> Result<()> { db.ingest_raw_event(event) }
}

async fn create_media_chunk(collector: &DesktopCollector, db: &AppDb) -> Result<()> {
    let settings = db.get_settings()?;
    let started_at = now();
    let id = Uuid::new_v4().to_string();
    let chunk_dir = db.media_cache_dir().join(format!("chunk-{}", id));
    fs::create_dir_all(&chunk_dir)?;
    let manifest_path = chunk_dir.join("manifest.json");
    let chunk = MediaChunk {
        id: id.clone(),
        display_id: "all-displays".into(),
        started_at: started_at.clone(),
        ended_at: None,
        path: chunk_dir.display().to_string(),
        fps: settings.fps,
        status: "recording".into(),
    };
    db.upsert_media_chunk(&chunk)?;

    let duration_secs = (settings.chunk_minutes.max(1) * 60) as u64;
    let effective_fps = settings.fps.clamp(0.1, 1.0);
    let frame_interval = Duration::from_millis((1000.0 / effective_fps) as u64);
    let deadline = Instant::now() + Duration::from_secs(duration_secs);
    let mut frames = Vec::new();
    let mut errors = Vec::new();
    let mut frame_index = 0usize;
    while collector.running.load(Ordering::SeqCst) && Instant::now() < deadline {
        let frame_path = chunk_dir.join(format!("frame-{frame_index:06}.bmp"));
        match capture_virtual_screen_bmp(&frame_path) {
            Ok(()) => frames.push(json!({
                "index": frame_index,
                "capturedAt": now(),
                "path": frame_path.display().to_string(),
            })),
            Err(err) => {
                errors.push(json!({ "at": now(), "error": err.to_string() }));
                if frame_index == 0 {
                    fs::write(&frame_path, b"ScreenUse screen capture failed; metadata-only analysis will continue.")?;
                }
            }
        }
        frame_index += 1;
        sleep(frame_interval).await;
    }
    let ended_at = now();
    let payload = json!({
        "id": id,
        "kind": "screenuse-media-chunk",
        "format": "bmp-frame-directory",
        "captureScope": settings.capture_scope,
        "fps": effective_fps,
        "startedAt": started_at,
        "endedAt": ended_at,
        "frames": frames,
        "errors": errors,
        "note": "低频屏幕帧只保存在本地临时缓存；AI/规则分析完成后会自动删除或按缓存上限清理。",
    });
    fs::write(&manifest_path, serde_json::to_vec_pretty(&payload)?)?;
    let chunk = MediaChunk {
        id: id.clone(),
        display_id: "all-displays".into(),
        started_at: started_at.clone(),
        ended_at: Some(ended_at.clone()),
        path: chunk_dir.display().to_string(),
        fps: effective_fps,
        status: "pending-analysis".into(),
    };
    db.upsert_media_chunk(&chunk)?;
    let job = AnalysisJob { id: Uuid::new_v4().to_string(), chunk_ids: vec![id], metadata_range: TimeRange { started_at: started_at.clone(), ended_at }, mode: settings.analysis_timing, retry_count: 0, status: "pending".into(), error: None };
    db.create_analysis_job(&job)?;
    db.cleanup_media_cache()?;
    Ok(())
}

#[cfg(windows)]
unsafe fn process_image_path(pid: u32) -> Result<String> {
    use windows::core::PWSTR;
    use windows::Win32::Foundation::{CloseHandle, BOOL};
    use windows::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_FORMAT, PROCESS_QUERY_LIMITED_INFORMATION};

    let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, BOOL(0), pid)?;
    let mut buf = vec![0u16; 32768];
    let mut size = buf.len() as u32;
    let result = QueryFullProcessImageNameW(handle, PROCESS_NAME_FORMAT(0), PWSTR(buf.as_mut_ptr()), &mut size);
    let _ = CloseHandle(handle);
    result?;
    Ok(String::from_utf16_lossy(&buf[..size as usize]))
}

#[cfg(windows)]
fn capture_foreground_event() -> Result<RawActivityEvent> {
    use std::path::PathBuf;
    use windows::Win32::System::SystemInformation::GetTickCount;
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId};

    unsafe {
        let hwnd = GetForegroundWindow();
        let len = GetWindowTextLengthW(hwnd).max(0) as usize;
        let mut buf = vec![0u16; len + 1];
        let copied = if !buf.is_empty() { GetWindowTextW(hwnd, &mut buf) } else { 0 };
        let title = String::from_utf16_lossy(&buf[..copied.max(0) as usize]);
        let mut pid = 0u32;
        let _thread = GetWindowThreadProcessId(hwnd, Some(&mut pid));
        let exe_path = process_image_path(pid).ok();
        let app_name = exe_path.as_ref()
            .and_then(|p| PathBuf::from(p).file_name().map(|s| s.to_string_lossy().to_string()))
            .unwrap_or_else(|| format!("pid:{}", pid));

        let mut last_input = LASTINPUTINFO { cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
        let idle_seconds = if GetLastInputInfo(&mut last_input).as_bool() {
            let tick = GetTickCount();
            tick.saturating_sub(last_input.dwTime) as u64 / 1000
        } else { 0 };

        Ok(RawActivityEvent {
            id: Uuid::new_v4().to_string(),
            source: "windows-foreground".into(),
            timestamp: now(),
            app: Some(app_name),
            window_title: Some(title),
            url: None,
            file_path: exe_path.clone(),
            workspace: None,
            input_stats: InputStats { idle_seconds, ..Default::default() },
            metadata: json!({ "pid": pid, "exePath": exe_path, "platform": "windows" }),
        })
    }
}

#[cfg(not(windows))]
fn capture_foreground_event() -> Result<RawActivityEvent> {
    Ok(RawActivityEvent {
        id: Uuid::new_v4().to_string(),
        source: "mock-foreground".into(),
        timestamp: now(),
        app: Some("mock-app".into()),
        window_title: Some("ScreenUse cross-platform collector placeholder".into()),
        url: None,
        file_path: None,
        workspace: None,
        input_stats: InputStats::default(),
        metadata: json!({ "platform": "non-windows" }),
    })
}

#[cfg(windows)]
fn capture_virtual_screen_bmp(path: &Path) -> Result<()> {
    use std::ffi::c_void;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Gdi::{
        CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits,
        ReleaseDC, SelectObject, SetStretchBltMode, StretchBlt, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
        CAPTUREBLT, DIB_RGB_COLORS, HALFTONE, HGDIOBJ, SRCCOPY,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
    };

    unsafe {
        let x = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let y = GetSystemMetrics(SM_YVIRTUALSCREEN);
        let width = GetSystemMetrics(SM_CXVIRTUALSCREEN);
        let height = GetSystemMetrics(SM_CYVIRTUALSCREEN);
        if width <= 0 || height <= 0 {
            return Err(anyhow!("virtual screen size is invalid: {width}x{height}"));
        }
        let target_width = width.min(1280).max(1);
        let target_height = ((height as f64) * (target_width as f64 / width as f64)).round().max(1.0) as i32;

        let hwnd = HWND(0 as _);
        let screen_dc = GetDC(hwnd);
        if screen_dc.is_invalid() {
            return Err(anyhow!("GetDC failed"));
        }
        let mem_dc = CreateCompatibleDC(screen_dc);
        if mem_dc.is_invalid() {
            let _ = ReleaseDC(hwnd, screen_dc);
            return Err(anyhow!("CreateCompatibleDC failed"));
        }
        let bitmap = CreateCompatibleBitmap(screen_dc, target_width, target_height);
        if bitmap.is_invalid() {
            let _ = DeleteDC(mem_dc);
            let _ = ReleaseDC(hwnd, screen_dc);
            return Err(anyhow!("CreateCompatibleBitmap failed"));
        }
        let old_obj = SelectObject(mem_dc, HGDIOBJ(bitmap.0));
        let _ = SetStretchBltMode(mem_dc, HALFTONE);
        if !StretchBlt(mem_dc, 0, 0, target_width, target_height, screen_dc, x, y, width, height, SRCCOPY | CAPTUREBLT).as_bool() {
            let _ = SelectObject(mem_dc, old_obj);
            let _ = DeleteObject(HGDIOBJ(bitmap.0));
            let _ = DeleteDC(mem_dc);
            let _ = ReleaseDC(hwnd, screen_dc);
            return Err(anyhow!("StretchBlt virtual screen failed"));
        }

        let stride = ((target_width as usize * 3 + 3) / 4) * 4;
        let image_size = stride * target_height as usize;
        let mut bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: target_width,
                biHeight: -target_height,
                biPlanes: 1,
                biBitCount: 24,
                biCompression: BI_RGB.0,
                biSizeImage: image_size as u32,
                biXPelsPerMeter: 0,
                biYPelsPerMeter: 0,
                biClrUsed: 0,
                biClrImportant: 0,
            },
            ..Default::default()
        };
        let mut pixels = vec![0u8; image_size];
        let copied = GetDIBits(
            mem_dc,
            bitmap,
            0,
            target_height as u32,
            Some(pixels.as_mut_ptr() as *mut c_void),
            &mut bmi,
            DIB_RGB_COLORS,
        );
        let _ = SelectObject(mem_dc, old_obj);
        let _ = DeleteObject(HGDIOBJ(bitmap.0));
        let _ = DeleteDC(mem_dc);
        let _ = ReleaseDC(hwnd, screen_dc);
        if copied == 0 {
            return Err(anyhow!("GetDIBits returned 0 scan lines"));
        }
        write_bmp_file(path, target_width, target_height, &pixels, stride)
    }
}

#[cfg(not(windows))]
fn capture_virtual_screen_bmp(path: &Path) -> Result<()> {
    fs::write(path, b"ScreenUse metadata-only placeholder frame on non-Windows.")?;
    Ok(())
}

fn write_bmp_file(path: &Path, width: i32, height: i32, pixels: &[u8], stride: usize) -> Result<()> {
    let pixel_bytes = stride * height as usize;
    let file_size = 14 + 40 + pixel_bytes;
    let mut file = File::create(path)?;
    file.write_all(b"BM")?;
    file.write_all(&(file_size as u32).to_le_bytes())?;
    file.write_all(&0u16.to_le_bytes())?;
    file.write_all(&0u16.to_le_bytes())?;
    file.write_all(&(54u32).to_le_bytes())?;
    file.write_all(&(40u32).to_le_bytes())?;
    file.write_all(&width.to_le_bytes())?;
    file.write_all(&(-height).to_le_bytes())?;
    file.write_all(&(1u16).to_le_bytes())?;
    file.write_all(&(24u16).to_le_bytes())?;
    file.write_all(&(0u32).to_le_bytes())?;
    file.write_all(&(pixel_bytes as u32).to_le_bytes())?;
    file.write_all(&(0i32).to_le_bytes())?;
    file.write_all(&(0i32).to_le_bytes())?;
    file.write_all(&(0u32).to_le_bytes())?;
    file.write_all(&(0u32).to_le_bytes())?;
    file.write_all(pixels)?;
    Ok(())
}
