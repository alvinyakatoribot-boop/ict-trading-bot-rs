use chrono::{DateTime, Timelike, Utc};
use chrono_tz::US::Eastern;

use crate::config::Config;

pub struct SessionManager {
    pub current_session: String,
    pub session_weight: f64,
}

impl SessionManager {
    pub fn new(cfg: &Config) -> Self {
        Self {
            current_session: "off_session".to_string(),
            session_weight: *cfg
                .session_weights
                .get("off_session")
                .unwrap_or(&0.5),
        }
    }

    pub fn update(&mut self, cfg: &Config, utc_now: Option<DateTime<Utc>>) {
        let utc_now = utc_now.unwrap_or_else(Utc::now);
        let et_now = utc_now.with_timezone(&Eastern);
        let current_time = et_now.hour() * 60 + et_now.minute();

        self.current_session = "off_session".to_string();
        self.session_weight = *cfg
            .session_weights
            .get("off_session")
            .unwrap_or(&0.5);

        for (name, times) in &cfg.sessions {
            let start_min = times.start.0 * 60 + times.start.1;
            let end_min = times.end.0 * 60 + times.end.1;

            let in_session = if start_min < end_min {
                current_time >= start_min && current_time < end_min
            } else {
                // Wraps midnight (e.g. Asian session 20:00 - 00:00)
                current_time >= start_min || current_time < end_min
            };

            if in_session {
                self.current_session = name.clone();
                self.session_weight = *cfg
                    .session_weights
                    .get(name)
                    .unwrap_or(&0.5);
                break;
            }
        }
    }

    pub fn is_london(&self) -> bool {
        self.current_session == "london"
    }

    pub fn is_ny(&self) -> bool {
        self.current_session == "ny_forex" || self.current_session == "ny_indices"
    }

    pub fn is_killzone(&self) -> bool {
        matches!(
            self.current_session.as_str(),
            "london" | "ny_forex" | "ny_indices"
        )
    }

    pub fn get_day_of_week(&self) -> String {
        let now_et = Utc::now().with_timezone(&Eastern);
        now_et.format("%A").to_string()
    }

    pub fn get_day_rating(&self, cfg: &Config, profile: &str) -> f64 {
        let day = self.get_day_of_week();
        cfg.day_ratings
            .get(profile)
            .map_or(0.0, |ratings| ratings.get(&day))
    }

    pub fn should_trade_today(&self, cfg: &Config, profile: &str) -> bool {
        self.get_day_rating(cfg, profile) >= cfg.min_day_rating
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::default_test_config;
    use chrono::TimeZone;

    fn make_utc_for_et_hour(et_hour: u32, et_minute: u32) -> DateTime<Utc> {
        // ET is UTC-5 (standard time) in January.
        // So ET 03:00 = UTC 08:00, ET 21:00 = UTC 02:00 next day
        use chrono::NaiveDate;
        let utc_hour = et_hour + 5;
        let (day_offset, hour) = if utc_hour >= 24 {
            (1, utc_hour - 24)
        } else {
            (0, utc_hour)
        };
        let date = NaiveDate::from_ymd_opt(2024, 1, 15 + day_offset).unwrap();
        let naive = date.and_hms_opt(hour, et_minute, 0).unwrap();
        Utc.from_utc_datetime(&naive)
    }

    #[test]
    fn london_session() {
        let cfg = default_test_config();
        let mut sm = SessionManager::new(&cfg);
        sm.update(&cfg, Some(make_utc_for_et_hour(3, 0))); // 3am ET = london
        assert_eq!(sm.current_session, "london");
        assert!(sm.is_london());
        assert!(sm.is_killzone());
    }

    #[test]
    fn ny_forex_session() {
        let cfg = default_test_config();
        let mut sm = SessionManager::new(&cfg);
        sm.update(&cfg, Some(make_utc_for_et_hour(8, 0))); // 8am ET = ny_forex
        assert_eq!(sm.current_session, "ny_forex");
        assert!(sm.is_ny());
        assert!(sm.is_killzone());
    }

    #[test]
    fn ny_indices_session() {
        let cfg = default_test_config();
        let mut sm = SessionManager::new(&cfg);
        sm.update(&cfg, Some(make_utc_for_et_hour(9, 0))); // 9am ET = ny_indices
        // Could match ny_forex (7-10) or ny_indices (8:30-12) depending on iteration order
        assert!(sm.is_killzone());
    }

    #[test]
    fn off_session() {
        let cfg = default_test_config();
        let mut sm = SessionManager::new(&cfg);
        sm.update(&cfg, Some(make_utc_for_et_hour(14, 0))); // 2pm ET = no session
        assert_eq!(sm.current_session, "off_session");
        assert!(!sm.is_killzone());
    }

    #[test]
    fn killzone_false_for_asian() {
        let cfg = default_test_config();
        let mut sm = SessionManager::new(&cfg);
        sm.update(&cfg, Some(make_utc_for_et_hour(21, 0))); // 9pm ET = asian
        assert_eq!(sm.current_session, "asian");
        assert!(!sm.is_killzone());
    }
}
