use crate::models::GlobalSettings;
use serde_json::Value;

const KEYS: &[&str] = &[
    "masterVolume",
    "normalizeEnabled",
    "autoplayDefault",
    "announceNowPlaying",
    "autoLeaveWhenEmpty",
    "emptyVoiceForced",
    "autoLeaveDelaySeconds",
    "emptyVoicePolicy",
    "cacheLimitGb",
    "logRetentionDays",
    "sponsorblockRemove",
    "tweakFfmpegFastStart",
    "tweakFfmpegDirectOutput",
    "voiceBitrateKbps",
];

pub fn public_settings(settings: &GlobalSettings) -> Value {
    let mut value = serde_json::to_value(settings).unwrap();
    value
        .as_object_mut()
        .unwrap()
        .retain(|key, _| KEYS.contains(&key.as_str()));
    value
}

pub fn apply(current: &GlobalSettings, patch: &Value) -> Result<GlobalSettings, String> {
    let fields = patch.as_object().ok_or("설정 객체가 필요해요.")?;
    if fields.keys().any(|key| !KEYS.contains(&key.as_str())) {
        return Err("이 화면에서 바꿀 수 없는 설정이 포함됐어요.".into());
    }
    let mut value = serde_json::to_value(current).unwrap();
    value.as_object_mut().unwrap().extend(fields.clone());
    let mut settings: GlobalSettings =
        serde_json::from_value(value).map_err(|_| "설정 값의 형식이 올바르지 않아요.")?;
    settings.master_volume = settings.master_volume.clamp(0, 200);
    settings.auto_leave_delay_seconds = settings.auto_leave_delay_seconds.clamp(5, 3600);
    settings.cache_limit_gb = settings.cache_limit_gb.clamp(1, 4096);
    settings.log_retention_days = settings.log_retention_days.clamp(1, 3650);
    settings.voice_bitrate_kbps = settings.voice_bitrate_kbps.clamp(32, 128);
    Ok(settings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn owner_settings_exclude_secrets_and_only_patch_public_fields() {
        let current = GlobalSettings {
            preferred_browser_profile: "private-profile".into(),
            cookie_file_path: Some("private-cookie-path".into()),
            ..Default::default()
        };
        let shown = public_settings(&current);
        assert!(!shown.to_string().contains("private"));
        let changed = apply(&current, &json!({ "masterVolume": 999 })).unwrap();
        assert_eq!(changed.master_volume, 200);
        assert_eq!(changed.cookie_file_path, current.cookie_file_path);
        assert_eq!(changed.normalize_enabled, current.normalize_enabled);
        assert!(apply(&current, &json!({ "cookieFilePath": "other" })).is_err());
        assert!(apply(&current, &json!({ "normalizeEnabled": "true" })).is_err());
        assert!(apply(&current, &json!({ "emptyVoicePolicy": "unknown" })).is_err());
    }
}
