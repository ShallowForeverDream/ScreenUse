use crate::models::{SleepDebtDay, SleepDebtSummary};
use chrono::{Datelike, Duration, NaiveDate, Weekday};
use std::collections::HashMap;

pub(crate) const START_DATE_SETTING_KEY: &str = "sleep_debt_start_date_v1";
pub(crate) const TARGET_MIGRATION_KEY: &str = "migration_sleep_target_v1";
pub(crate) const CATEGORY_NAME: &str = "休息";
pub(crate) const PROJECT_NAME: &str = "睡眠";
pub(crate) const PROJECT_ID: &str = "system-rest-sleep";
pub(crate) const PROJECT_SOURCE: &str = "sleep-system";
pub(crate) const NAP_TASK_ID: &str = "system-rest-sleep-nap";
pub(crate) const NAP_TASK_SOURCE: &str = "sleep-system:nap";
pub(crate) const NAP_TASK_TITLE: &str = "午睡";
pub(crate) const SLEEP_TASK_ID: &str = "system-rest-sleep-night";
pub(crate) const SLEEP_TASK_SOURCE: &str = "sleep-system:sleep";
pub(crate) const SLEEP_TASK_TITLE: &str = "睡觉";

const DAILY_TARGET_SECONDS: f64 = 8.0 * 60.0 * 60.0;
const MONDAY_DEBT_SECONDS: f64 = 3.0 * 60.0 * 60.0;
const FIRST_LAYER_GROWTH: f64 = 1.5;
const SECOND_LAYER_GROWTH: f64 = 2.0;
const SECOND_LAYER_REPAYMENT_EFFECT: f64 = 1.0 / 3.0;
const HEATMAP_HISTORY_DAYS: usize = 371;

#[derive(Debug, Clone)]
struct FirstLayerDebt {
    origin: NaiveDate,
    amount: f64,
    compounds_daily: bool,
}

#[derive(Debug, Clone)]
struct SecondLayerDebt {
    entered: NaiveDate,
    amount: f64,
}

pub(crate) fn calculate(
    started_on: NaiveDate,
    as_of: NaiveDate,
    sleep_seconds_by_day: &HashMap<NaiveDate, u64>,
) -> SleepDebtSummary {
    if started_on > as_of {
        return empty_summary(as_of);
    }

    let mut first_layer = Vec::<FirstLayerDebt>::new();
    let mut second_layer = Vec::<SecondLayerDebt>::new();
    let mut days = Vec::<SleepDebtDay>::new();
    let mut day = started_on;

    while day <= as_of {
        if day > started_on {
            for debt in &mut first_layer {
                if debt.compounds_daily {
                    debt.amount *= FIRST_LAYER_GROWTH;
                }
            }
        }

        let mut retained = Vec::with_capacity(first_layer.len());
        for debt in first_layer.drain(..) {
            if (day - debt.origin).num_days() >= 7 {
                second_layer.push(SecondLayerDebt {
                    entered: day,
                    amount: debt.amount,
                });
            } else {
                retained.push(debt);
            }
        }
        first_layer = retained;

        for debt in &mut second_layer {
            let age_days = (day - debt.entered).num_days();
            if age_days > 0 && age_days % 7 == 0 {
                debt.amount *= SECOND_LAYER_GROWTH;
            }
        }

        let monday_debt_added_seconds = if day.weekday() == Weekday::Mon {
            first_layer.push(FirstLayerDebt {
                origin: day,
                amount: MONDAY_DEBT_SECONDS,
                compounds_daily: false,
            });
            MONDAY_DEBT_SECONDS as u64
        } else {
            0
        };

        let slept = sleep_seconds_by_day.get(&day).copied().unwrap_or_default() as f64;
        if slept < DAILY_TARGET_SECONDS {
            first_layer.push(FirstLayerDebt {
                origin: day,
                amount: DAILY_TARGET_SECONDS - slept,
                compounds_daily: true,
            });
        } else if slept > DAILY_TARGET_SECONDS {
            let mut surplus = slept - DAILY_TARGET_SECONDS;
            repay_oldest_first(&mut first_layer, &mut surplus);
            if first_layer.is_empty() && surplus > 0.0 {
                let mut effective_repayment = surplus * SECOND_LAYER_REPAYMENT_EFFECT;
                repay_oldest_second_layer(&mut second_layer, &mut effective_repayment);
            }
        }

        first_layer.retain(|debt| debt.amount >= 0.5);
        second_layer.retain(|debt| debt.amount >= 0.5);
        days.push(SleepDebtDay {
            date: day.to_string(),
            sleep_seconds: slept.round() as u64,
            daily_target_seconds: DAILY_TARGET_SECONDS as u64,
            daily_shortfall_seconds: (DAILY_TARGET_SECONDS - slept).max(0.0).round() as u64,
            daily_surplus_seconds: (slept - DAILY_TARGET_SECONDS).max(0.0).round() as u64,
            monday_debt_added_seconds,
            first_layer_seconds: rounded_sum(first_layer.iter().map(|debt| debt.amount)),
            second_layer_seconds: rounded_sum(second_layer.iter().map(|debt| debt.amount)),
            periods: Vec::new(),
        });
        day += Duration::days(1);
    }

    if days.len() > HEATMAP_HISTORY_DAYS {
        days.drain(..days.len() - HEATMAP_HISTORY_DAYS);
    }

    let first_layer_seconds = rounded_sum(first_layer.iter().map(|debt| debt.amount));
    let second_layer_seconds = rounded_sum(second_layer.iter().map(|debt| debt.amount));
    SleepDebtSummary {
        as_of_date: as_of.to_string(),
        started_on: started_on.to_string(),
        daily_target_seconds: DAILY_TARGET_SECONDS as u64,
        sleep_seconds_today: sleep_seconds_by_day
            .get(&as_of)
            .copied()
            .unwrap_or_default(),
        first_layer_seconds,
        second_layer_seconds,
        total_seconds: first_layer_seconds.saturating_add(second_layer_seconds),
        days,
    }
}

fn repay_oldest_first(debts: &mut Vec<FirstLayerDebt>, available: &mut f64) {
    debts.sort_by_key(|debt| (debt.origin, !debt.compounds_daily));
    for debt in debts.iter_mut() {
        if *available <= 0.0 {
            break;
        }
        let repaid = debt.amount.min(*available);
        debt.amount -= repaid;
        *available -= repaid;
    }
    debts.retain(|debt| debt.amount >= 0.5);
}

fn repay_oldest_second_layer(debts: &mut Vec<SecondLayerDebt>, available: &mut f64) {
    debts.sort_by_key(|debt| debt.entered);
    for debt in debts.iter_mut() {
        if *available <= 0.0 {
            break;
        }
        let repaid = debt.amount.min(*available);
        debt.amount -= repaid;
        *available -= repaid;
    }
    debts.retain(|debt| debt.amount >= 0.5);
}

fn rounded_sum(values: impl Iterator<Item = f64>) -> u64 {
    values.sum::<f64>().max(0.0).round() as u64
}

fn empty_summary(as_of: NaiveDate) -> SleepDebtSummary {
    SleepDebtSummary {
        as_of_date: as_of.to_string(),
        started_on: as_of.to_string(),
        daily_target_seconds: DAILY_TARGET_SECONDS as u64,
        sleep_seconds_today: 0,
        first_layer_seconds: 0,
        second_layer_seconds: 0,
        total_seconds: 0,
        days: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(value: &str) -> NaiveDate {
        NaiveDate::parse_from_str(value, "%Y-%m-%d").expect("valid date")
    }

    fn sleep_days(values: &[(&str, f64)]) -> HashMap<NaiveDate, u64> {
        values
            .iter()
            .map(|(day, hours)| (date(day), (hours * 3_600.0) as u64))
            .collect()
    }

    #[test]
    fn daily_deficit_grows_by_half_each_day() {
        let sleep = sleep_days(&[
            ("2026-07-21", 7.0),
            ("2026-07-22", 8.0),
            ("2026-07-23", 8.0),
            ("2026-07-24", 8.0),
        ]);
        let result = calculate(date("2026-07-21"), date("2026-07-24"), &sleep);
        assert_eq!(result.first_layer_seconds, (3.375 * 3_600.0) as u64);
        assert_eq!(result.second_layer_seconds, 0);
    }

    #[test]
    fn monday_debt_does_not_compound_daily() {
        let sleep = sleep_days(&[
            ("2026-07-20", 8.0),
            ("2026-07-21", 8.0),
            ("2026-07-22", 8.0),
            ("2026-07-23", 8.0),
        ]);
        let result = calculate(date("2026-07-20"), date("2026-07-23"), &sleep);
        assert_eq!(result.first_layer_seconds, 3 * 3_600);
    }

    #[test]
    fn same_day_surplus_repays_compounding_shortfall_before_fixed_monday_debt() {
        let monday = date("2026-07-20");
        let mut debts = vec![
            FirstLayerDebt {
                origin: monday,
                amount: 3.0 * 3_600.0,
                compounds_daily: false,
            },
            FirstLayerDebt {
                origin: monday,
                amount: 4.0 * 3_600.0,
                compounds_daily: true,
            },
        ];
        let mut surplus = 2.0 * 3_600.0;

        repay_oldest_first(&mut debts, &mut surplus);

        assert_eq!(surplus, 0.0);
        assert_eq!(debts.len(), 2);
        assert_eq!(debts[0].amount, 2.0 * 3_600.0);
        assert!(debts[0].compounds_daily);
        assert_eq!(debts[1].amount, 3.0 * 3_600.0);
        assert!(!debts[1].compounds_daily);
    }

    #[test]
    fn fixed_monday_debt_stays_flat_while_remaining_shortfall_compounds() {
        let sleep = sleep_days(&[
            ("2026-07-20", 4.0),
            ("2026-07-21", 10.0),
            ("2026-07-22", 8.0),
        ]);

        let result = calculate(date("2026-07-20"), date("2026-07-22"), &sleep);

        assert_eq!(result.first_layer_seconds, 9 * 3_600);
        assert_eq!(result.second_layer_seconds, 0);
    }

    #[test]
    fn surplus_repays_first_layer_before_second_layer() {
        let sleep = sleep_days(&[("2026-07-21", 7.0), ("2026-07-22", 10.0)]);
        let result = calculate(date("2026-07-21"), date("2026-07-22"), &sleep);
        assert_eq!(result.first_layer_seconds, 0);
        assert_eq!(result.second_layer_seconds, 0);
    }

    #[test]
    fn old_first_layer_debt_moves_to_second_layer_after_seven_days() {
        let mut values = vec![("2026-07-21", 7.0)];
        values.extend([
            ("2026-07-22", 8.0),
            ("2026-07-23", 8.0),
            ("2026-07-24", 8.0),
            ("2026-07-25", 8.0),
            ("2026-07-26", 8.0),
            ("2026-07-27", 11.0),
            ("2026-07-28", 8.0),
        ]);
        let result = calculate(date("2026-07-21"), date("2026-07-28"), &sleep_days(&values));
        assert_eq!(result.first_layer_seconds, 3 * 3_600);
        assert_eq!(result.second_layer_seconds, 45_309);
    }

    #[test]
    fn second_layer_doubles_weekly_and_surplus_has_one_third_effect() {
        let mut values = vec![("2026-07-21", 7.0)];
        for day in 22..=31 {
            values.push((
                match day {
                    22 => "2026-07-22",
                    23 => "2026-07-23",
                    24 => "2026-07-24",
                    25 => "2026-07-25",
                    26 => "2026-07-26",
                    27 => "2026-07-27",
                    28 => "2026-07-28",
                    29 => "2026-07-29",
                    30 => "2026-07-30",
                    _ => "2026-07-31",
                },
                8.0,
            ));
        }
        values.extend([
            ("2026-08-01", 8.0),
            ("2026-08-02", 8.0),
            ("2026-08-03", 8.0),
            ("2026-08-04", 8.0),
        ]);
        let result = calculate(date("2026-07-21"), date("2026-08-04"), &sleep_days(&values));
        assert_eq!(result.second_layer_seconds, 133_819);
        assert_eq!(result.first_layer_seconds, 3 * 3_600);

        let repayment = calculate(
            date("2026-07-21"),
            date("2026-07-30"),
            &sleep_days(&[
                ("2026-07-21", 7.0),
                ("2026-07-22", 8.0),
                ("2026-07-23", 8.0),
                ("2026-07-24", 8.0),
                ("2026-07-25", 8.0),
                ("2026-07-26", 8.0),
                ("2026-07-27", 8.0),
                ("2026-07-28", 8.0),
                ("2026-07-29", 11.0),
                ("2026-07-30", 11.0),
            ]),
        );
        assert_eq!(repayment.first_layer_seconds, 0);
        assert_eq!(repayment.second_layer_seconds, 57_909);
    }
}
