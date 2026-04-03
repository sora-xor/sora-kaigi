use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TargetPlatform {
    WebChromium,
    WebSafari,
    WebFirefox,
    MacOS,
    IOS,
    IPadOS,
    VisionOS,
    Windows,
    Android,
    Linux,
}

impl TargetPlatform {
    pub fn is_web(self) -> bool {
        matches!(
            self,
            TargetPlatform::WebChromium | TargetPlatform::WebSafari | TargetPlatform::WebFirefox
        )
    }

    pub fn is_native(self) -> bool {
        !self.is_web()
    }
}

pub const TARGET_PLATFORMS: [TargetPlatform; 10] = [
    TargetPlatform::WebChromium,
    TargetPlatform::WebSafari,
    TargetPlatform::WebFirefox,
    TargetPlatform::MacOS,
    TargetPlatform::IOS,
    TargetPlatform::IPadOS,
    TargetPlatform::VisionOS,
    TargetPlatform::Windows,
    TargetPlatform::Android,
    TargetPlatform::Linux,
];

pub const WEB_FALLBACK_PLATFORMS: [TargetPlatform; 3] = [
    TargetPlatform::WebChromium,
    TargetPlatform::WebSafari,
    TargetPlatform::WebFirefox,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AppSurface {
    Native,
    Web,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlatformContract {
    pub platform: TargetPlatform,
    pub app_surface: AppSurface,
    pub parity: ParityTargets,
    pub security: SecurityTargets,
    pub media: MediaTargets,
    pub recording: RecordingTargets,
    pub fallback: FallbackTargets,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParityTargets {
    pub wallet_host_identity: bool,
    pub guest_policy_controls: bool,
    pub host_cohost_participant_roles: bool,
    pub moderation_commands: bool,
    pub waiting_room_and_room_lock: bool,
    pub roster_chat_reactions_hand_raise: bool,
    pub reconnect_and_session_continuity: bool,
    pub participant_scale_target: u16,
    pub accessibility_shortcuts_and_labels: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecurityTargets {
    pub e2ee_default: bool,
    pub signed_high_risk_actions: bool,
    pub replay_resistance: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaTargets {
    pub camera_mic_speaker: bool,
    pub screen_share: bool,
    pub hdr_on_supported_devices: bool,
    pub sdr_tonemap_fallback: bool,
    pub active_profile_indicator: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordingTargets {
    pub local_recording_policy_controlled: bool,
    pub host_and_participant_recording_paths: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FallbackTargets {
    pub web_fallback_required: bool,
    pub fallback_platforms: Vec<TargetPlatform>,
}

pub fn platform_contract(platform: TargetPlatform) -> PlatformContract {
    PlatformContract {
        platform,
        app_surface: if platform.is_web() {
            AppSurface::Web
        } else {
            AppSurface::Native
        },
        parity: ParityTargets {
            wallet_host_identity: true,
            guest_policy_controls: true,
            host_cohost_participant_roles: true,
            moderation_commands: true,
            waiting_room_and_room_lock: true,
            roster_chat_reactions_hand_raise: true,
            reconnect_and_session_continuity: true,
            participant_scale_target: 500,
            accessibility_shortcuts_and_labels: true,
        },
        security: SecurityTargets {
            e2ee_default: true,
            signed_high_risk_actions: true,
            replay_resistance: true,
        },
        media: MediaTargets {
            camera_mic_speaker: true,
            screen_share: true,
            hdr_on_supported_devices: true,
            sdr_tonemap_fallback: true,
            active_profile_indicator: true,
        },
        recording: RecordingTargets {
            local_recording_policy_controlled: true,
            host_and_participant_recording_paths: true,
        },
        fallback: if platform.is_native() {
            FallbackTargets {
                web_fallback_required: true,
                fallback_platforms: WEB_FALLBACK_PLATFORMS.to_vec(),
            }
        } else {
            FallbackTargets {
                web_fallback_required: false,
                fallback_platforms: Vec::new(),
            }
        },
    }
}

pub fn all_platform_contracts() -> Vec<PlatformContract> {
    TARGET_PLATFORMS
        .into_iter()
        .map(platform_contract)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn all_target_platforms_have_contracts() {
        let contracts = all_platform_contracts();
        assert_eq!(contracts.len(), TARGET_PLATFORMS.len());
        let seen = contracts
            .iter()
            .map(|contract| contract.platform)
            .collect::<HashSet<_>>();
        assert_eq!(seen.len(), TARGET_PLATFORMS.len());
    }

    #[test]
    fn native_platforms_require_web_fallback() {
        let expected_fallbacks = WEB_FALLBACK_PLATFORMS.into_iter().collect::<HashSet<_>>();
        for contract in all_platform_contracts() {
            if contract.platform.is_native() {
                assert!(contract.fallback.web_fallback_required);
                assert_eq!(
                    contract
                        .fallback
                        .fallback_platforms
                        .iter()
                        .copied()
                        .collect::<HashSet<_>>(),
                    expected_fallbacks
                );
            } else {
                assert!(!contract.fallback.web_fallback_required);
                assert!(contract.fallback.fallback_platforms.is_empty());
            }
        }
    }

    #[test]
    fn all_platforms_enforce_security_baseline() {
        for contract in all_platform_contracts() {
            assert!(contract.security.e2ee_default);
            assert!(contract.security.signed_high_risk_actions);
            assert!(contract.security.replay_resistance);
        }
    }

    #[test]
    fn all_platforms_define_media_hdr_and_sdr_fallback() {
        for contract in all_platform_contracts() {
            assert!(contract.media.camera_mic_speaker);
            assert!(contract.media.screen_share);
            assert!(contract.media.hdr_on_supported_devices);
            assert!(contract.media.sdr_tonemap_fallback);
            assert!(contract.media.active_profile_indicator);
        }
    }

    #[test]
    fn all_platforms_target_full_feature_parity() {
        for contract in all_platform_contracts() {
            assert!(contract.parity.wallet_host_identity);
            assert!(contract.parity.guest_policy_controls);
            assert!(contract.parity.host_cohost_participant_roles);
            assert!(contract.parity.moderation_commands);
            assert!(contract.parity.waiting_room_and_room_lock);
            assert!(contract.parity.roster_chat_reactions_hand_raise);
            assert!(contract.parity.reconnect_and_session_continuity);
            assert_eq!(contract.parity.participant_scale_target, 500);
            assert!(contract.parity.accessibility_shortcuts_and_labels);
            assert!(contract.recording.local_recording_policy_controlled);
            assert!(contract.recording.host_and_participant_recording_paths);
        }
    }

    #[test]
    fn windows_is_native_with_web_fallback() {
        let windows = platform_contract(TargetPlatform::Windows);
        assert_eq!(windows.app_surface, AppSurface::Native);
        assert!(windows.fallback.web_fallback_required);
        assert_eq!(
            windows.fallback.fallback_platforms.len(),
            WEB_FALLBACK_PLATFORMS.len()
        );
    }
}
