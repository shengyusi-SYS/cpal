// AAudio's actual ID uses the same integer namespace as AudioDeviceInfo.getId().
pub(super) fn actual_device_id(id: i32) -> Option<String> {
    (id > 0).then(|| id.to_string())
}

#[cfg(test)]
mod tests {
    use super::actual_device_id;

    #[test]
    fn output_route_observation_android_id_preserves_audio_device_info_id() {
        assert_eq!(actual_device_id(42).as_deref(), Some("42"));
        assert_eq!(actual_device_id(i32::MAX).as_deref(), Some("2147483647"));
    }

    #[test]
    fn output_route_observation_android_unspecified_is_unknown() {
        assert_eq!(actual_device_id(0), None);
        assert_eq!(actual_device_id(-1), None);
    }
}
