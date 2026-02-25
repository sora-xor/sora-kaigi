import Foundation

enum SessionConnectionState: String, Codable, Equatable, CaseIterable {
    case disconnected
    case connecting
    case connected
    case degraded
    case fallbackActive
    case error

    var label: String {
        switch self {
        case .disconnected: return "Disconnected"
        case .connecting: return "Connecting"
        case .connected: return "Connected"
        case .degraded: return "Degraded"
        case .fallbackActive: return "FallbackActive"
        case .error: return "Error"
        }
    }

    var isOnline: Bool {
        switch self {
        case .connected, .degraded:
            return true
        default:
            return false
        }
    }
}

enum SessionErrorCategory: String, Codable, Equatable {
    case protocolFailure
    case policyFailure
    case transportFailure
}

struct SessionError: Codable, Equatable {
    var category: SessionErrorCategory
    var code: String
    var message: String
    var atMs: Int64
}

enum ParticipantRole: String, Codable, Equatable {
    case host
    case coHost = "co_host"
    case participant
    case guest
}

struct MeetingParticipant: Codable, Equatable, Identifiable {
    var id: String
    var displayName: String
    var role: ParticipantRole
    var muted: Bool
    var videoEnabled: Bool
    var shareEnabled: Bool
    var waitingRoom: Bool
}

struct RoleChange: Codable, Equatable {
    var participantID: String
    var role: ParticipantRole
}

enum ModerationAction: String, Codable, Equatable {
    case mute
    case videoOff
    case stopShare
    case kick
    case admitFromWaiting
    case denyFromWaiting
}

enum MediaProfileKind: String, Codable, Equatable {
    case sdr
    case hdr
}

struct MediaProfileState: Codable, Equatable {
    var preferredProfile: MediaProfileKind
    var negotiatedProfile: MediaProfileKind
    var colorPrimaries: String
    var transferFunction: String
    var codec: String

    static let `default` = MediaProfileState(
        preferredProfile: .sdr,
        negotiatedProfile: .sdr,
        colorPrimaries: "bt709",
        transferFunction: "gamma",
        codec: "h264"
    )
}

enum RecordingState: String, Codable, Equatable {
    case stopped
    case started
}

enum GuestPolicy: String, Codable, Equatable {
    case open
    case inviteOnly = "invite_only"
    case blocked
}

struct RecordingNoticeState: Codable, Equatable {
    var state: RecordingState
    var policyBasis: String
    var issuedBy: String

    static let `default` = RecordingNoticeState(
        state: .stopped,
        policyBasis: "policy-default",
        issuedBy: "system"
    )
}

enum PaymentSettlementStatus: String, Codable, Equatable {
    case notRequired
    case pending
    case settled
    case blocked
}

struct PaymentState: Codable, Equatable {
    var required: Bool
    var destination: String?
    var settlementStatus: PaymentSettlementStatus

    static let `default` = PaymentState(required: false, destination: nil, settlementStatus: .notRequired)
}

struct E2EEState: Codable, Equatable {
    var currentEpoch: Int
    var lastAckEpoch: Int

    static let `default` = E2EEState(currentEpoch: 0, lastAckEpoch: 0)
}

struct FallbackState: Codable, Equatable {
    var active: Bool
    var reason: String?
    var activatedAtMs: Int64?
    var recoveredAtMs: Int64?
    var lastRtoMs: Int64?

    static let `default` = FallbackState(active: false, reason: nil, activatedAtMs: nil, recoveredAtMs: nil, lastRtoMs: nil)
}

struct MeetingSessionState: Equatable {
    var config: MeetingConfig
    var connectionState: SessionConnectionState
    var handshakeComplete: Bool
    var resumeToken: String?
    var participants: [String: MeetingParticipant]
    var permissionSnapshots: [String: PermissionsSnapshotFrame]
    var presenceSequence: Int64
    var roomLocked: Bool
    var waitingRoomEnabled: Bool
    var guestPolicy: GuestPolicy
    var e2eeRequired: Bool
    var maxParticipants: Int
    var policyEpoch: Int
    var mediaProfile: MediaProfileState
    var recordingNotice: RecordingNoticeState
    var e2ee: E2EEState
    var payment: PaymentState
    var fallback: FallbackState
    var lastError: SessionError?

    static func initial(config: MeetingConfig) -> MeetingSessionState {
        MeetingSessionState(
            config: config,
            connectionState: .disconnected,
            handshakeComplete: false,
            resumeToken: nil,
            participants: [:],
            permissionSnapshots: [:],
            presenceSequence: 0,
            roomLocked: false,
            waitingRoomEnabled: false,
            guestPolicy: .open,
            e2eeRequired: true,
            maxParticipants: 300,
            policyEpoch: 0,
            mediaProfile: .default,
            recordingNotice: .default,
            e2ee: .default,
            payment: .default,
            fallback: .default,
            lastError: nil
        )
    }
}

enum MeetingSessionEvent: Equatable {
    case connectRequested
    case transportConnected
    case transportDisconnected(reason: String)
    case transportFailure(message: String)
    case frameReceived(MeetingProtocolFrame)
    case frameSendFailed(message: String)
    case manualDisconnected
    case fallbackActivated(reason: String)
    case fallbackRecovered
    case configUpdated(MeetingConfig)
}

// MARK: - Wire Frames

enum MeetingFrameKind: String, Codable, Equatable {
    case handshake
    case handshakeAck
    case participantPresenceDelta
    case roleGrant
    case roleRevoke
    case permissionsSnapshot
    case moderationSigned
    case sessionPolicy
    case deviceCapability
    case mediaProfileNegotiation
    case recordingNotice
    case e2eeKeyEpoch
    case keyRotationAck
    case paymentPolicy
    case paymentSettlement
    case ping
    case pong
    case error
}

struct HandshakeFrame: Codable, Equatable {
    var roomID: String
    var participantID: String
    var participantName: String
    var walletIdentity: String? = nil
    var resumeToken: String?
    var preferredProfile: MediaProfileKind
    var hdrCapture: Bool
    var hdrRender: Bool
    var sentAtMs: Int64
}

struct HandshakeAckFrame: Codable, Equatable {
    var sessionID: String
    var resumeToken: String
    var acceptedAtMs: Int64
}

struct ParticipantPresenceDeltaFrame: Codable, Equatable {
    var joined: [MeetingParticipant]
    var left: [String]
    var roleChanges: [RoleChange]
    var sequence: Int64
}

struct RoleGrantFrame: Codable, Equatable {
    var targetParticipantID: String
    var role: ParticipantRole
    var grantedBy: String
    var signature: String
    var issuedAtMs: Int64
}

struct RoleRevokeFrame: Codable, Equatable {
    var targetParticipantID: String
    var role: ParticipantRole
    var revokedBy: String
    var signature: String
    var issuedAtMs: Int64
}

struct PermissionsSnapshotFrame: Codable, Equatable {
    var participantID: String
    var effectivePermissions: [String]
    var epoch: Int
}

struct ModerationSignedFrame: Codable, Equatable {
    var sentAtMs: Int64
    var targetParticipantID: String
    var action: ModerationAction
    var issuedBy: String
    var signature: String
}

struct SessionPolicyFrame: Codable, Equatable {
    var roomLock: Bool
    var waitingRoomEnabled: Bool
    var recordingPolicy: RecordingState
    var guestPolicy: GuestPolicy
    var e2eeRequired: Bool
    var maxParticipants: Int
    var policyEpoch: Int
    var updatedBy: String
    var signature: String
}

struct DeviceCapabilityFrame: Codable, Equatable {
    var participantID: String
    var codecs: [String]
    var hdrCapture: Bool
    var hdrRender: Bool
    var maxStreams: Int
    var updatedAtMs: Int64
}

struct MediaProfileNegotiationFrame: Codable, Equatable {
    var participantID: String
    var preferredProfile: MediaProfileKind
    var negotiatedProfile: MediaProfileKind
    var colorPrimaries: String
    var transferFunction: String
    var codec: String
    var epoch: Int
}

struct RecordingNoticeFrame: Codable, Equatable {
    var participantID: String
    var state: RecordingState
    var mode: String
    var policyBasis: String
    var issuedAtMs: Int64
    var issuedBy: String
}

struct E2EEKeyEpochFrame: Codable, Equatable {
    var participantID: String
    var epoch: Int
    var publicKey: String
    var signature: String
    var issuedAtMs: Int64
}

struct KeyRotationAckFrame: Codable, Equatable {
    var participantID: String
    var ackEpoch: Int
    var receivedAtMs: Int64
}

struct PaymentPolicyFrame: Codable, Equatable {
    var required: Bool
    var destinationAccount: String?
}

struct PaymentSettlementFrame: Codable, Equatable {
    var status: PaymentSettlementStatus
}

struct PingFrame: Codable, Equatable {
    var sentAtMs: Int64
}

struct ErrorFrame: Codable, Equatable {
    var category: SessionErrorCategory
    var code: String
    var message: String
}

struct MeetingProtocolFrame: Codable, Equatable {
    var kind: MeetingFrameKind
    var handshake: HandshakeFrame?
    var handshakeAck: HandshakeAckFrame?
    var presenceDelta: ParticipantPresenceDeltaFrame?
    var roleGrant: RoleGrantFrame?
    var roleRevoke: RoleRevokeFrame?
    var permissionsSnapshot: PermissionsSnapshotFrame?
    var moderationSigned: ModerationSignedFrame?
    var sessionPolicy: SessionPolicyFrame?
    var deviceCapability: DeviceCapabilityFrame?
    var mediaProfileNegotiation: MediaProfileNegotiationFrame?
    var recordingNotice: RecordingNoticeFrame?
    var e2eeKeyEpoch: E2EEKeyEpochFrame?
    var keyRotationAck: KeyRotationAckFrame?
    var paymentPolicy: PaymentPolicyFrame?
    var paymentSettlement: PaymentSettlementFrame?
    var ping: PingFrame?
    var pong: PingFrame?
    var error: ErrorFrame?

    static func handshake(_ value: HandshakeFrame) -> MeetingProtocolFrame {
        MeetingProtocolFrame(kind: .handshake, handshake: value)
    }

    static func handshakeAck(_ value: HandshakeAckFrame) -> MeetingProtocolFrame {
        MeetingProtocolFrame(kind: .handshakeAck, handshakeAck: value)
    }

    static func ping(at ms: Int64) -> MeetingProtocolFrame {
        MeetingProtocolFrame(kind: .ping, ping: PingFrame(sentAtMs: ms))
    }

    static func pong(at ms: Int64) -> MeetingProtocolFrame {
        MeetingProtocolFrame(kind: .pong, pong: PingFrame(sentAtMs: ms))
    }

    static func error(category: SessionErrorCategory, code: String, message: String) -> MeetingProtocolFrame {
        MeetingProtocolFrame(kind: .error, error: ErrorFrame(category: category, code: code, message: message))
    }

    init(
        kind: MeetingFrameKind,
        handshake: HandshakeFrame? = nil,
        handshakeAck: HandshakeAckFrame? = nil,
        presenceDelta: ParticipantPresenceDeltaFrame? = nil,
        roleGrant: RoleGrantFrame? = nil,
        roleRevoke: RoleRevokeFrame? = nil,
        permissionsSnapshot: PermissionsSnapshotFrame? = nil,
        moderationSigned: ModerationSignedFrame? = nil,
        sessionPolicy: SessionPolicyFrame? = nil,
        deviceCapability: DeviceCapabilityFrame? = nil,
        mediaProfileNegotiation: MediaProfileNegotiationFrame? = nil,
        recordingNotice: RecordingNoticeFrame? = nil,
        e2eeKeyEpoch: E2EEKeyEpochFrame? = nil,
        keyRotationAck: KeyRotationAckFrame? = nil,
        paymentPolicy: PaymentPolicyFrame? = nil,
        paymentSettlement: PaymentSettlementFrame? = nil,
        ping: PingFrame? = nil,
        pong: PingFrame? = nil,
        error: ErrorFrame? = nil
    ) {
        self.kind = kind
        self.handshake = handshake
        self.handshakeAck = handshakeAck
        self.presenceDelta = presenceDelta
        self.roleGrant = roleGrant
        self.roleRevoke = roleRevoke
        self.permissionsSnapshot = permissionsSnapshot
        self.moderationSigned = moderationSigned
        self.sessionPolicy = sessionPolicy
        self.deviceCapability = deviceCapability
        self.mediaProfileNegotiation = mediaProfileNegotiation
        self.recordingNotice = recordingNotice
        self.e2eeKeyEpoch = e2eeKeyEpoch
        self.keyRotationAck = keyRotationAck
        self.paymentPolicy = paymentPolicy
        self.paymentSettlement = paymentSettlement
        self.ping = ping
        self.pong = pong
        self.error = error
    }
}

enum MeetingProtocolCodec {
    private static let encoder: JSONEncoder = {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.sortedKeys]
        return encoder
    }()

    private static let decoder = JSONDecoder()

    static func encode(_ frame: MeetingProtocolFrame) throws -> String {
        let data = try encoder.encode(frame)
        guard let text = String(data: data, encoding: .utf8) else {
            throw CodecError.invalidUTF8
        }
        return text
    }

    static func decode(_ text: String) throws -> MeetingProtocolFrame {
        guard let data = text.data(using: .utf8) else {
            throw CodecError.invalidUTF8
        }
        if let canonicalData = canonicalizedJSONData(data),
           let frame = try? decoder.decode(MeetingProtocolFrame.self, from: canonicalData) {
            return frame
        }
        return try decoder.decode(MeetingProtocolFrame.self, from: data)
    }

    private static func canonicalizedJSONData(_ data: Data) -> Data? {
        guard let root = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            return nil
        }
        let canonicalRoot = canonicalizedRoot(root)
        return try? JSONSerialization.data(withJSONObject: canonicalRoot, options: [])
    }

    private static func canonicalizedRoot(_ root: [String: Any]) -> [String: Any] {
        guard let kindRaw = stringValue(root["kind"]) else { return root }

        let kind = canonicalKind(kindRaw)
        var canonical = root
        canonical["kind"] = kind

        guard let payloadKey = payloadKey(for: kind) else {
            return canonical
        }

        guard let payload = canonicalizedPayload(kind: kind, root: root) else {
            return canonical
        }

        canonical[payloadKey] = payload
        return canonical
    }

    private static func payloadKey(for kind: String) -> String? {
        switch kind {
        case "handshake":
            return "handshake"
        case "handshakeAck":
            return "handshakeAck"
        case "participantPresenceDelta":
            return "presenceDelta"
        case "roleGrant":
            return "roleGrant"
        case "roleRevoke":
            return "roleRevoke"
        case "permissionsSnapshot":
            return "permissionsSnapshot"
        case "moderationSigned":
            return "moderationSigned"
        case "sessionPolicy":
            return "sessionPolicy"
        case "deviceCapability":
            return "deviceCapability"
        case "mediaProfileNegotiation":
            return "mediaProfileNegotiation"
        case "recordingNotice":
            return "recordingNotice"
        case "e2eeKeyEpoch":
            return "e2eeKeyEpoch"
        case "keyRotationAck":
            return "keyRotationAck"
        case "paymentPolicy":
            return "paymentPolicy"
        case "paymentSettlement":
            return "paymentSettlement"
        case "ping":
            return "ping"
        case "pong":
            return "pong"
        case "error":
            return "error"
        default:
            return nil
        }
    }

    private static func canonicalizedPayload(kind: String, root: [String: Any]) -> [String: Any]? {
        switch kind {
        case "handshake":
            return canonicalizedHandshake(payloadOrRoot(root, keys: ["handshake"]))
        case "handshakeAck":
            return canonicalizedHandshakeAck(payloadOrRoot(root, keys: ["handshakeAck", "handshake_ack"]))
        case "participantPresenceDelta":
            return canonicalizedPresenceDelta(payloadOrRoot(root, keys: ["presenceDelta", "participantPresenceDelta", "participant_presence_delta"]))
        case "roleGrant":
            return canonicalizedRoleGrant(payloadOrRoot(root, keys: ["roleGrant", "role_grant"]))
        case "roleRevoke":
            return canonicalizedRoleRevoke(payloadOrRoot(root, keys: ["roleRevoke", "role_revoke"]))
        case "permissionsSnapshot":
            return canonicalizedPermissionsSnapshot(payloadOrRoot(root, keys: ["permissionsSnapshot", "permissions_snapshot"]))
        case "moderationSigned":
            return canonicalizedModerationSigned(payloadOrRoot(root, keys: ["moderationSigned", "moderation_signed"]))
        case "sessionPolicy":
            return canonicalizedSessionPolicy(payloadOrRoot(root, keys: ["sessionPolicy", "session_policy"]))
        case "deviceCapability":
            return canonicalizedDeviceCapability(payloadOrRoot(root, keys: ["deviceCapability", "device_capability"]))
        case "mediaProfileNegotiation":
            return canonicalizedMediaProfileNegotiation(payloadOrRoot(root, keys: ["mediaProfileNegotiation", "media_profile_negotiation"]))
        case "recordingNotice":
            return canonicalizedRecordingNotice(payloadOrRoot(root, keys: ["recordingNotice", "recording_notice"]))
        case "e2eeKeyEpoch":
            return canonicalizedE2EEKeyEpoch(payloadOrRoot(root, keys: ["e2eeKeyEpoch", "e2ee_key_epoch"]))
        case "keyRotationAck":
            return canonicalizedKeyRotationAck(payloadOrRoot(root, keys: ["keyRotationAck", "key_rotation_ack"]))
        case "paymentPolicy":
            return canonicalizedPaymentPolicy(payloadOrRoot(root, keys: ["paymentPolicy", "payment_policy"]))
        case "paymentSettlement":
            return canonicalizedPaymentSettlement(payloadOrRoot(root, keys: ["paymentSettlement", "payment_settlement"]))
        case "ping":
            return canonicalizedPing(payloadOrRoot(root, keys: ["ping"]))
        case "pong":
            return canonicalizedPing(payloadOrRoot(root, keys: ["pong"]))
        case "error":
            return canonicalizedError(payloadOrRoot(root, keys: ["error"]))
        default:
            return nil
        }
    }

    private static func payloadOrRoot(_ root: [String: Any], keys: [String]) -> [String: Any] {
        for key in keys {
            if let payload = root[key] as? [String: Any] {
                return payload
            }
        }
        return root
    }

    private static func canonicalizedHandshake(_ payload: [String: Any]) -> [String: Any] {
        var canonical = canonicalizedObject(payload, mapping: [
            ("roomID", ["roomID", "roomId", "room_id"]),
            ("participantID", ["participantID", "participantId", "participant_id"]),
            ("participantName", ["participantName", "participant_name"]),
            ("walletIdentity", ["walletIdentity", "wallet_identity"]),
            ("resumeToken", ["resumeToken", "resume_token"])
        ])
        if let preferredProfile = canonicalMediaProfile(firstValue(payload, keys: ["preferredProfile", "preferred_profile"])) {
            canonical["preferredProfile"] = preferredProfile
        }
        if let hdrCapture = boolValue(firstValue(payload, keys: ["hdrCapture", "hdr_capture"])) {
            canonical["hdrCapture"] = hdrCapture
        }
        if let hdrRender = boolValue(firstValue(payload, keys: ["hdrRender", "hdr_render"])) {
            canonical["hdrRender"] = hdrRender
        }
        if let sentAtMs = int64Value(firstValue(payload, keys: ["sentAtMs", "sent_at_ms"])) {
            canonical["sentAtMs"] = sentAtMs
        }
        return canonical
    }

    private static func canonicalizedHandshakeAck(_ payload: [String: Any]) -> [String: Any] {
        var canonical = canonicalizedObject(payload, mapping: [
            ("sessionID", ["sessionID", "sessionId", "session_id"]),
            ("resumeToken", ["resumeToken", "resume_token"])
        ])
        if let acceptedAtMs = int64Value(firstValue(payload, keys: ["acceptedAtMs", "accepted_at_ms"])) {
            canonical["acceptedAtMs"] = acceptedAtMs
        }
        return canonical
    }

    private static func canonicalizedPresenceDelta(_ payload: [String: Any]) -> [String: Any] {
        var canonical: [String: Any] = [:]
        let joined = (firstValue(payload, keys: ["joined"]) as? [[String: Any]]) ?? []
        canonical["joined"] = joined.map(canonicalizedParticipant)
        let left = (firstValue(payload, keys: ["left"]) as? [Any]) ?? []
        canonical["left"] = left.compactMap(stringValue)
        let roleChanges = (firstValue(payload, keys: ["roleChanges", "role_changes"]) as? [[String: Any]]) ?? []
        canonical["roleChanges"] = roleChanges.map(canonicalizedRoleChange)
        if let sequence = int64Value(firstValue(payload, keys: ["sequence"])) {
            canonical["sequence"] = sequence
        }
        return canonical
    }

    private static func canonicalizedParticipant(_ participant: [String: Any]) -> [String: Any] {
        var canonical = canonicalizedObject(participant, mapping: [
            ("id", ["id"]),
            ("displayName", ["displayName", "display_name"])
        ])
        if let role = canonicalRole(firstValue(participant, keys: ["role"])) {
            canonical["role"] = role
        }
        if let muted = boolValue(firstValue(participant, keys: ["muted"])) {
            canonical["muted"] = muted
        }
        if let videoEnabled = boolValue(firstValue(participant, keys: ["videoEnabled", "video_enabled"])) {
            canonical["videoEnabled"] = videoEnabled
        }
        if let shareEnabled = boolValue(firstValue(participant, keys: ["shareEnabled", "share_enabled"])) {
            canonical["shareEnabled"] = shareEnabled
        }
        if let waitingRoom = boolValue(firstValue(participant, keys: ["waitingRoom", "waiting_room"])) {
            canonical["waitingRoom"] = waitingRoom
        }
        return canonical
    }

    private static func canonicalizedRoleChange(_ roleChange: [String: Any]) -> [String: Any] {
        var canonical = canonicalizedObject(roleChange, mapping: [
            ("participantID", ["participantID", "participantId", "participant_id"])
        ])
        if let role = canonicalRole(firstValue(roleChange, keys: ["role"])) {
            canonical["role"] = role
        }
        return canonical
    }

    private static func canonicalizedRoleGrant(_ payload: [String: Any]) -> [String: Any] {
        var canonical = canonicalizedObject(payload, mapping: [
            ("targetParticipantID", ["targetParticipantID", "targetParticipantId", "target_participant_id"]),
            ("grantedBy", ["grantedBy", "granted_by"]),
            ("signature", ["signature"])
        ])
        if let role = canonicalRole(firstValue(payload, keys: ["role"])) {
            canonical["role"] = role
        }
        if let issuedAtMs = int64Value(firstValue(payload, keys: ["issuedAtMs", "issued_at_ms"])) {
            canonical["issuedAtMs"] = issuedAtMs
        }
        return canonical
    }

    private static func canonicalizedRoleRevoke(_ payload: [String: Any]) -> [String: Any] {
        var canonical = canonicalizedObject(payload, mapping: [
            ("targetParticipantID", ["targetParticipantID", "targetParticipantId", "target_participant_id"]),
            ("revokedBy", ["revokedBy", "revoked_by"]),
            ("signature", ["signature"])
        ])
        if let role = canonicalRole(firstValue(payload, keys: ["role"])) {
            canonical["role"] = role
        }
        if let issuedAtMs = int64Value(firstValue(payload, keys: ["issuedAtMs", "issued_at_ms"])) {
            canonical["issuedAtMs"] = issuedAtMs
        }
        return canonical
    }

    private static func canonicalizedPermissionsSnapshot(_ payload: [String: Any]) -> [String: Any] {
        var canonical = canonicalizedObject(payload, mapping: [
            ("participantID", ["participantID", "participantId", "participant_id"])
        ])
        let permissions = (firstValue(payload, keys: ["effectivePermissions", "effective_permissions"]) as? [Any]) ?? []
        canonical["effectivePermissions"] = permissions.compactMap(stringValue)
        if let epoch = intValue(firstValue(payload, keys: ["epoch"])) {
            canonical["epoch"] = epoch
        }
        return canonical
    }

    private static func canonicalizedModerationSigned(_ payload: [String: Any]) -> [String: Any] {
        var canonical = canonicalizedObject(payload, mapping: [
            ("targetParticipantID", ["targetParticipantID", "targetParticipantId", "target_participant_id"]),
            ("issuedBy", ["issuedBy", "issued_by"]),
            ("signature", ["signature"])
        ])
        if let sentAtMs = int64Value(firstValue(payload, keys: ["sentAtMs", "sent_at_ms"])) {
            canonical["sentAtMs"] = sentAtMs
        }
        if let action = canonicalModerationAction(firstValue(payload, keys: ["action"])) {
            canonical["action"] = action
        }
        return canonical
    }

    private static func canonicalizedSessionPolicy(_ payload: [String: Any]) -> [String: Any] {
        var canonical = canonicalizedObject(payload, mapping: [
            ("signature", ["signature"]),
            ("updatedBy", ["updatedBy", "updated_by"])
        ])
        if let roomLock = boolValue(firstValue(payload, keys: ["roomLock", "room_lock"])) {
            canonical["roomLock"] = roomLock
        }
        if let waitingRoomEnabled = boolValue(firstValue(payload, keys: ["waitingRoomEnabled", "waiting_room_enabled"])) {
            canonical["waitingRoomEnabled"] = waitingRoomEnabled
        }
        if let recordingPolicy = canonicalRecordingState(firstValue(payload, keys: ["recordingPolicy", "recording_policy"])) {
            canonical["recordingPolicy"] = recordingPolicy
        }
        if let guestPolicy = canonicalGuestPolicy(firstValue(payload, keys: ["guestPolicy", "guest_policy"])) {
            canonical["guestPolicy"] = guestPolicy
        }
        if let e2eeRequired = boolValue(firstValue(payload, keys: ["e2eeRequired", "e2ee_required"])) {
            canonical["e2eeRequired"] = e2eeRequired
        }
        if let maxParticipants = intValue(firstValue(payload, keys: ["maxParticipants", "max_participants"])) {
            canonical["maxParticipants"] = maxParticipants
        }
        if let policyEpoch = intValue(firstValue(payload, keys: ["policyEpoch", "policy_epoch"])) {
            canonical["policyEpoch"] = policyEpoch
        }
        return canonical
    }

    private static func canonicalizedDeviceCapability(_ payload: [String: Any]) -> [String: Any] {
        var canonical = canonicalizedObject(payload, mapping: [
            ("participantID", ["participantID", "participantId", "participant_id"])
        ])
        let codecs = (firstValue(payload, keys: ["codecs"]) as? [Any]) ?? []
        canonical["codecs"] = codecs.compactMap(stringValue)
        if let hdrCapture = boolValue(firstValue(payload, keys: ["hdrCapture", "hdr_capture"])) {
            canonical["hdrCapture"] = hdrCapture
        }
        if let hdrRender = boolValue(firstValue(payload, keys: ["hdrRender", "hdr_render"])) {
            canonical["hdrRender"] = hdrRender
        }
        if let maxStreams = intValue(firstValue(payload, keys: ["maxStreams", "max_streams"])) {
            canonical["maxStreams"] = maxStreams
        }
        if let updatedAtMs = int64Value(firstValue(payload, keys: ["updatedAtMs", "updated_at_ms"])) {
            canonical["updatedAtMs"] = updatedAtMs
        }
        return canonical
    }

    private static func canonicalizedMediaProfileNegotiation(_ payload: [String: Any]) -> [String: Any] {
        var canonical = canonicalizedObject(payload, mapping: [
            ("participantID", ["participantID", "participantId", "participant_id"]),
            ("colorPrimaries", ["colorPrimaries", "color_primaries"]),
            ("transferFunction", ["transferFunction", "transfer_function"]),
            ("codec", ["codec"])
        ])
        if let preferredProfile = canonicalMediaProfile(firstValue(payload, keys: ["preferredProfile", "preferred_profile"])) {
            canonical["preferredProfile"] = preferredProfile
        }
        if let negotiatedProfile = canonicalMediaProfile(firstValue(payload, keys: ["negotiatedProfile", "negotiated_profile"])) {
            canonical["negotiatedProfile"] = negotiatedProfile
        }
        if let epoch = intValue(firstValue(payload, keys: ["epoch"])) {
            canonical["epoch"] = epoch
        }
        return canonical
    }

    private static func canonicalizedRecordingNotice(_ payload: [String: Any]) -> [String: Any] {
        var canonical = canonicalizedObject(payload, mapping: [
            ("participantID", ["participantID", "participantId", "participant_id"]),
            ("mode", ["mode"]),
            ("policyBasis", ["policyBasis", "policy_basis"]),
            ("issuedBy", ["issuedBy", "issued_by"])
        ])
        if let state = canonicalRecordingState(firstValue(payload, keys: ["state"])) {
            canonical["state"] = state
        }
        if let issuedAtMs = int64Value(firstValue(payload, keys: ["issuedAtMs", "issued_at_ms"])) {
            canonical["issuedAtMs"] = issuedAtMs
        }
        return canonical
    }

    private static func canonicalizedE2EEKeyEpoch(_ payload: [String: Any]) -> [String: Any] {
        var canonical = canonicalizedObject(payload, mapping: [
            ("participantID", ["participantID", "participantId", "participant_id"]),
            ("publicKey", ["publicKey", "public_key"]),
            ("signature", ["signature"])
        ])
        if let epoch = intValue(firstValue(payload, keys: ["epoch"])) {
            canonical["epoch"] = epoch
        }
        if let issuedAtMs = int64Value(firstValue(payload, keys: ["issuedAtMs", "issued_at_ms"])) {
            canonical["issuedAtMs"] = issuedAtMs
        }
        return canonical
    }

    private static func canonicalizedKeyRotationAck(_ payload: [String: Any]) -> [String: Any] {
        var canonical = canonicalizedObject(payload, mapping: [
            ("participantID", ["participantID", "participantId", "participant_id"])
        ])
        if let ackEpoch = intValue(firstValue(payload, keys: ["ackEpoch", "ack_epoch"])) {
            canonical["ackEpoch"] = ackEpoch
        }
        if let receivedAtMs = int64Value(firstValue(payload, keys: ["receivedAtMs", "received_at_ms"])) {
            canonical["receivedAtMs"] = receivedAtMs
        }
        return canonical
    }

    private static func canonicalizedPaymentPolicy(_ payload: [String: Any]) -> [String: Any] {
        var canonical = canonicalizedObject(payload, mapping: [
            ("destinationAccount", ["destinationAccount", "destination_account"])
        ])
        if let required = boolValue(firstValue(payload, keys: ["required"])) {
            canonical["required"] = required
        }
        return canonical
    }

    private static func canonicalizedPaymentSettlement(_ payload: [String: Any]) -> [String: Any] {
        var canonical: [String: Any] = [:]
        if let status = canonicalPaymentSettlementStatus(firstValue(payload, keys: ["status"])) {
            canonical["status"] = status
        }
        return canonical
    }

    private static func canonicalizedPing(_ payload: [String: Any]) -> [String: Any] {
        var canonical: [String: Any] = [:]
        if let sentAtMs = int64Value(firstValue(payload, keys: ["sentAtMs", "sent_at_ms"])) {
            canonical["sentAtMs"] = sentAtMs
        }
        return canonical
    }

    private static func canonicalizedError(_ payload: [String: Any]) -> [String: Any] {
        var canonical = canonicalizedObject(payload, mapping: [
            ("code", ["code"]),
            ("message", ["message"])
        ])
        if let category = canonicalErrorCategory(firstValue(payload, keys: ["category"])) {
            canonical["category"] = category
        }
        return canonical
    }

    private static func canonicalizedObject(_ source: [String: Any], mapping: [(String, [String])]) -> [String: Any] {
        var canonical: [String: Any] = [:]
        for (field, aliases) in mapping {
            if let value = firstValue(source, keys: aliases) {
                canonical[field] = value
            }
        }
        return canonical
    }

    private static func firstValue(_ source: [String: Any], keys: [String]) -> Any? {
        for key in keys {
            if let value = source[key] {
                return value
            }
        }
        return nil
    }

    private static func stringValue(_ value: Any?) -> String? {
        switch value {
        case let string as String:
            return string
        case let number as NSNumber:
            return number.stringValue
        default:
            return nil
        }
    }

    private static func boolValue(_ value: Any?) -> Bool? {
        switch value {
        case let bool as Bool:
            return bool
        case let number as NSNumber:
            return number.boolValue
        case let string as String:
            switch string.lowercased() {
            case "true":
                return true
            case "false":
                return false
            default:
                return nil
            }
        default:
            return nil
        }
    }

    private static func intValue(_ value: Any?) -> Int? {
        switch value {
        case let int as Int:
            return int
        case let int64 as Int64:
            return Int(int64)
        case let double as Double:
            return Int(double)
        case let number as NSNumber:
            return number.intValue
        case let string as String:
            return Int(string)
        default:
            return nil
        }
    }

    private static func int64Value(_ value: Any?) -> Int64? {
        switch value {
        case let int64 as Int64:
            return int64
        case let int as Int:
            return Int64(int)
        case let double as Double:
            return Int64(double)
        case let number as NSNumber:
            return number.int64Value
        case let string as String:
            return Int64(string)
        default:
            return nil
        }
    }

    private static func canonicalKind(_ value: String) -> String {
        switch value {
        case "handshake":
            return "handshake"
        case "handshakeAck", "handshake_ack":
            return "handshakeAck"
        case "participantPresenceDelta", "participant_presence_delta":
            return "participantPresenceDelta"
        case "roleGrant", "role_grant":
            return "roleGrant"
        case "roleRevoke", "role_revoke":
            return "roleRevoke"
        case "permissionsSnapshot", "permissions_snapshot":
            return "permissionsSnapshot"
        case "moderationSigned", "moderation_signed":
            return "moderationSigned"
        case "sessionPolicy", "session_policy":
            return "sessionPolicy"
        case "deviceCapability", "device_capability":
            return "deviceCapability"
        case "mediaProfileNegotiation", "media_profile_negotiation":
            return "mediaProfileNegotiation"
        case "recordingNotice", "recording_notice":
            return "recordingNotice"
        case "e2eeKeyEpoch", "e2ee_key_epoch":
            return "e2eeKeyEpoch"
        case "keyRotationAck", "key_rotation_ack":
            return "keyRotationAck"
        case "paymentPolicy", "payment_policy":
            return "paymentPolicy"
        case "paymentSettlement", "payment_settlement":
            return "paymentSettlement"
        case "ping":
            return "ping"
        case "pong":
            return "pong"
        case "error":
            return "error"
        default:
            return value
        }
    }

    private static func canonicalRole(_ value: Any?) -> String? {
        guard let role = stringValue(value) else { return nil }
        switch role {
        case "coHost", "co_host":
            return "co_host"
        case "host", "participant", "guest":
            return role
        default:
            return role
        }
    }

    private static func canonicalModerationAction(_ value: Any?) -> String? {
        guard let action = stringValue(value) else { return nil }
        switch action {
        case "video_off", "videoOff":
            return "videoOff"
        case "stop_share", "stopShare":
            return "stopShare"
        case "admit_from_waiting", "admitFromWaiting":
            return "admitFromWaiting"
        case "deny_from_waiting", "denyFromWaiting":
            return "denyFromWaiting"
        default:
            return action
        }
    }

    private static func canonicalMediaProfile(_ value: Any?) -> String? {
        guard let profile = stringValue(value)?.lowercased() else { return nil }
        switch profile {
        case "hdr":
            return "hdr"
        case "sdr":
            return "sdr"
        default:
            return profile
        }
    }

    private static func canonicalRecordingState(_ value: Any?) -> String? {
        guard let state = stringValue(value)?.lowercased() else { return nil }
        switch state {
        case "started":
            return "started"
        case "stopped":
            return "stopped"
        default:
            return state
        }
    }

    private static func canonicalGuestPolicy(_ value: Any?) -> String? {
        guard let policy = stringValue(value) else { return nil }
        switch policy {
        case "inviteOnly", "invite_only":
            return "invite_only"
        case "open", "blocked":
            return policy
        default:
            return policy
        }
    }

    private static func canonicalPaymentSettlementStatus(_ value: Any?) -> String? {
        guard let status = stringValue(value) else { return nil }
        switch status {
        case "notRequired", "not_required":
            return "notRequired"
        case "pending", "settled", "blocked":
            return status
        default:
            return status
        }
    }

    private static func canonicalErrorCategory(_ value: Any?) -> String? {
        guard let category = stringValue(value) else { return nil }
        switch category {
        case "policyFailure", "policy_failure":
            return "policyFailure"
        case "transportFailure", "transport_failure":
            return "transportFailure"
        case "protocolFailure", "protocol_failure":
            return "protocolFailure"
        default:
            return category
        }
    }

    enum CodecError: Error {
        case invalidUTF8
    }
}
