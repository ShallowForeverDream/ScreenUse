use crate::db::AppDb;
use crate::models::WorkSession;
use anyhow::{anyhow, Result};
use std::fs;
use std::path::PathBuf;

pub trait ExportProvider {
    fn export(&self, db: &AppDb, sessions: &[WorkSession]) -> Result<PathBuf>;
}

pub fn export_sessions(db: &AppDb, format: &str) -> Result<PathBuf> {
    let sessions = db.list_sessions(10_000)?;
    let path = match format.to_lowercase().as_str() {
        "csv" => CsvExport.export(db, &sessions)?,
        "excel" | "xls" => ExcelHtmlExport.export(db, &sessions)?,
        "markdown" | "md" => MarkdownExport.export(db, &sessions)?,
        other => return Err(anyhow!("unsupported export format: {other}")),
    };
    db.record_export(format, &path)?;
    Ok(path)
}

struct CsvExport;
struct ExcelHtmlExport;
struct MarkdownExport;

impl ExportProvider for CsvExport {
    fn export(&self, db: &AppDb, sessions: &[WorkSession]) -> Result<PathBuf> {
        let path = db.export_path("csv");
        let mut out = String::from("started_at,ended_at,project,task,category,summary,confidence,user_confirmed,source\n");
        for s in sessions {
            out.push_str(&format!("{},{},{},{},{},{},{:.2},{},{}\n", esc(&s.started_at), esc(&s.ended_at), esc(s.project_name.as_deref().unwrap_or("")), esc(s.task_title.as_deref().unwrap_or("")), esc(&s.category), esc(&s.summary), s.confidence, s.user_confirmed, esc(&s.source)));
        }
        fs::write(&path, out)?;
        Ok(path)
    }
}

impl ExportProvider for ExcelHtmlExport {
    fn export(&self, db: &AppDb, sessions: &[WorkSession]) -> Result<PathBuf> {
        let path = db.export_path("xls");
        let mut out = String::from("<html><meta charset='utf-8'><body><table border='1'><tr><th>开始</th><th>结束</th><th>项目</th><th>任务</th><th>分类</th><th>摘要</th><th>置信度</th><th>已确认</th></tr>");
        for s in sessions {
            out.push_str(&format!("<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{:.2}</td><td>{}</td></tr>", html(&s.started_at), html(&s.ended_at), html(s.project_name.as_deref().unwrap_or("")), html(s.task_title.as_deref().unwrap_or("")), html(&s.category), html(&s.summary), s.confidence, s.user_confirmed));
        }
        out.push_str("</table></body></html>");
        fs::write(&path, out)?;
        Ok(path)
    }
}

impl ExportProvider for MarkdownExport {
    fn export(&self, db: &AppDb, sessions: &[WorkSession]) -> Result<PathBuf> {
        let path = db.export_path("md");
        let mut out = String::from("# ScreenUse 时间复盘\n\n| 开始 | 结束 | 项目 | 任务 | 分类 | 摘要 | 置信度 |\n|---|---|---|---|---|---|---:|\n");
        for s in sessions {
            out.push_str(&format!("| {} | {} | {} | {} | {} | {} | {:.2} |\n", s.started_at, s.ended_at, md(s.project_name.as_deref().unwrap_or("")), md(s.task_title.as_deref().unwrap_or("")), md(&s.category), md(&s.summary), s.confidence));
        }
        fs::write(&path, out)?;
        Ok(path)
    }
}

fn esc(s: &str) -> String { format!("\"{}\"", s.replace('"', "\"\"").replace('\n', " ")) }
fn html(s: &str) -> String { s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;") }
fn md(s: &str) -> String { s.replace('|', "\\|").replace('\n', " ") }
