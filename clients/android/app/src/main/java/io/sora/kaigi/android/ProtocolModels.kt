package io.sora.kaigi.android

import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.intOrNull
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.longOrNull

enum class ConnectionPhase(val label: String, val online: Boolean) {
    Disconnected("Disconnected", false),
    Connecting("Connecting", false),
    Connected("Connected", true),
    Degraded("Degraded", true),
    FallbackActive("FallbackActive", false),
    Error("Error", false)
}

enum class SessionErrorCategory(val wire: String) {
    ProtocolFailure("protocol_failure"),
    PolicyFailure("policy_failure"),
    TransportFailure("transport_failure");

    companion object {
        fun fromWire(value: String?): SessionErrorCategory {
            return when (value) {
                PolicyFailure.wire, "policyFailure" -> PolicyFailure
                TransportFailure.wire, "transportFailure" -> TransportFailure
                else -> ProtocolFailure
            }
        }
    }
}

data class SessionError(
    val category: SessionErrorCategory,
    val code: String,
    val message: String,
    val atMs: Long
)

enum class ParticipantRole(val wire: String) {
    Host("host"),
    CoHost("co_host"),
    Participant("participant"),
    Guest("guest");

    companion object {
        fun fromWire(value: String?): ParticipantRole {
            if (value == "coHost") return CoHost
            return entries.firstOrNull { it.wire == value } ?: Participant
        }
    }
}

data class Participant(
    val id: String,
    val displayName: String,
    val role: ParticipantRole,
    val muted: Boolean,
    val videoEnabled: Boolean,
    val shareEnabled: Boolean,
    val waitingRoom: Boolean
)

data class RoleChange(
    val participantId: String,
    val role: ParticipantRole
)

enum class ModerationAction(val wire: String) {
    Mute("mute"),
    VideoOff("video_off"),
    StopShare("stop_share"),
    Kick("kick"),
    AdmitFromWaiting("admit_from_waiting"),
    DenyFromWaiting("deny_from_waiting");

    companion object {
        fun fromWire(value: String?): ModerationAction {
            return when (value) {
                "videoOff" -> VideoOff
                "stopShare" -> StopShare
                "admitFromWaiting" -> AdmitFromWaiting
                "denyFromWaiting" -> DenyFromWaiting
                else -> entries.firstOrNull { it.wire == value } ?: Mute
            }
        }
    }
}

enum class MediaProfile(val wire: String) {
    SDR("sdr"),
    HDR("hdr");

    companion object {
        fun fromWire(value: String?): MediaProfile {
            return entries.firstOrNull { it.wire == value } ?: SDR
        }
    }
}

data class MediaProfileState(
    val preferredProfile: MediaProfile,
    val negotiatedProfile: MediaProfile,
    val colorPrimaries: String,
    val transferFunction: String,
    val codec: String
) {
    companion object {
        val DEFAULT = MediaProfileState(
            preferredProfile = MediaProfile.SDR,
            negotiatedProfile = MediaProfile.SDR,
            colorPrimaries = "bt709",
            transferFunction = "gamma",
            codec = "h264"
        )
    }
}

enum class RecordingState(val wire: String) {
    Stopped("stopped"),
    Started("started");

    companion object {
        fun fromWire(value: String?): RecordingState {
            return entries.firstOrNull { it.wire == value } ?: Stopped
        }
    }
}

enum class GuestPolicy(val wire: String) {
    Open("open"),
    InviteOnly("invite_only"),
    Blocked("blocked");

    companion object {
        fun fromWire(value: String?): GuestPolicy {
            if (value == "inviteOnly") return InviteOnly
            return entries.firstOrNull { it.wire == value } ?: Open
        }
    }
}

data class PermissionSnapshot(
    val effectivePermissions: List<String>,
    val epoch: Int
)

enum class PaymentSettlementStatus(val wire: String) {
    NotRequired("not_required"),
    Pending("pending"),
    Settled("settled"),
    Blocked("blocked");

    companion object {
        fun fromWire(value: String?): PaymentSettlementStatus {
            if (value == "notRequired") return NotRequired
            return entries.firstOrNull { it.wire == value } ?: NotRequired
        }
    }
}

data class PaymentState(
    val required: Boolean,
    val destination: String?,
    val settlementStatus: PaymentSettlementStatus
) {
    companion object {
        val DEFAULT = PaymentState(
            required = false,
            destination = null,
            settlementStatus = PaymentSettlementStatus.NotRequired
        )
    }
}

data class E2eeState(
    val currentEpoch: Int,
    val lastAckEpoch: Int
) {
    companion object {
        val DEFAULT = E2eeState(currentEpoch = 0, lastAckEpoch = 0)
    }
}

data class FallbackState(
    val active: Boolean,
    val reason: String?,
    val activatedAtMs: Long?,
    val recoveredAtMs: Long?,
    val lastRtoMs: Long?
) {
    companion object {
        val DEFAULT = FallbackState(
            active = false,
            reason = null,
            activatedAtMs = null,
            recoveredAtMs = null,
            lastRtoMs = null
        )
    }
}

data class ProtocolSessionState(
    val config: MeetingConfig,
    val connectionPhase: ConnectionPhase,
    val handshakeComplete: Boolean,
    val resumeToken: String?,
    val participants: Map<String, Participant>,
    val permissionSnapshots: Map<String, PermissionSnapshot>,
    val presenceSequence: Long,
    val roomLocked: Boolean,
    val waitingRoomEnabled: Boolean,
    val guestPolicy: GuestPolicy,
    val e2eeRequired: Boolean,
    val maxParticipants: Int,
    val policyEpoch: Int,
    val mediaProfile: MediaProfileState,
    val recordingNotice: RecordingState,
    val e2eeState: E2eeState,
    val paymentState: PaymentState,
    val fallback: FallbackState,
    val lastError: SessionError?
) {
    companion object {
        fun initial(config: MeetingConfig): ProtocolSessionState {
            return ProtocolSessionState(
                config = config,
                connectionPhase = ConnectionPhase.Disconnected,
                handshakeComplete = false,
                resumeToken = null,
                participants = emptyMap(),
                permissionSnapshots = emptyMap(),
                presenceSequence = 0,
                roomLocked = false,
                waitingRoomEnabled = false,
                guestPolicy = GuestPolicy.Open,
                e2eeRequired = true,
                maxParticipants = 300,
                policyEpoch = 0,
                mediaProfile = MediaProfileState.DEFAULT,
                recordingNotice = RecordingState.Stopped,
                e2eeState = E2eeState.DEFAULT,
                paymentState = PaymentState.DEFAULT,
                fallback = FallbackState.DEFAULT,
                lastError = null
            )
        }
    }
}

sealed interface SessionCommand {
    data object Connect : SessionCommand
    data object Disconnect : SessionCommand
    data object Ping : SessionCommand
    data class Moderate(val action: ModerationAction, val targetParticipantId: String) : SessionCommand
    data object LifecycleForegrounded : SessionCommand
    data object LifecycleBackgrounded : SessionCommand
    data class ConnectivityChanged(val available: Boolean) : SessionCommand
    data object AudioInterruptionBegan : SessionCommand
    data class AudioInterruptionEnded(val shouldReconnect: Boolean = true) : SessionCommand
    data class AudioRouteChanged(val reason: String) : SessionCommand
}

sealed interface ProtocolEvent {
    data object ConnectRequested : ProtocolEvent
    data object TransportConnected : ProtocolEvent
    data class TransportDisconnected(val reason: String) : ProtocolEvent
    data class TransportFailure(val message: String) : ProtocolEvent
    data class FrameReceived(val frame: ProtocolFrame) : ProtocolEvent
    data class FrameSendFailed(val message: String) : ProtocolEvent
    data object ManualDisconnected : ProtocolEvent
    data class FallbackActivated(val reason: String) : ProtocolEvent
    data object FallbackRecovered : ProtocolEvent
    data class ConfigUpdated(val config: MeetingConfig) : ProtocolEvent
}

sealed interface ProtocolFrame {
    val kind: String

    data class Handshake(
        val roomId: String,
        val participantId: String,
        val participantName: String,
        val walletIdentity: String? = null,
        val resumeToken: String?,
        val preferredProfile: MediaProfile,
        val hdrCapture: Boolean,
        val hdrRender: Boolean,
        val sentAtMs: Long
    ) : ProtocolFrame {
        override val kind: String = "handshake"
    }

    data class HandshakeAck(
        val sessionId: String,
        val resumeToken: String,
        val acceptedAtMs: Long
    ) : ProtocolFrame {
        override val kind: String = "handshakeAck"
    }

    data class PresenceDelta(
        val joined: List<Participant>,
        val left: List<String>,
        val roleChanges: List<RoleChange>,
        val sequence: Long
    ) : ProtocolFrame {
        override val kind: String = "participantPresenceDelta"
    }

    data class RoleGrant(
        val targetParticipantId: String,
        val role: ParticipantRole,
        val grantedBy: String,
        val signature: String,
        val issuedAtMs: Long
    ) : ProtocolFrame {
        override val kind: String = "roleGrant"
    }

    data class RoleRevoke(
        val targetParticipantId: String,
        val role: ParticipantRole,
        val revokedBy: String,
        val signature: String,
        val issuedAtMs: Long
    ) : ProtocolFrame {
        override val kind: String = "roleRevoke"
    }

    data class PermissionsSnapshot(
        val participantId: String,
        val effectivePermissions: List<String>,
        val epoch: Int
    ) : ProtocolFrame {
        override val kind: String = "permissionsSnapshot"
    }

    data class ModerationSigned(
        val sentAtMs: Long,
        val targetParticipantId: String,
        val action: ModerationAction,
        val issuedBy: String,
        val signature: String
    ) : ProtocolFrame {
        override val kind: String = "moderationSigned"
    }

    data class SessionPolicy(
        val roomLock: Boolean,
        val waitingRoomEnabled: Boolean,
        val recordingPolicy: RecordingState,
        val guestPolicy: GuestPolicy,
        val e2eeRequired: Boolean,
        val maxParticipants: Int,
        val policyEpoch: Int,
        val updatedBy: String,
        val signature: String
    ) : ProtocolFrame {
        override val kind: String = "sessionPolicy"
    }

    data class DeviceCapability(
        val participantId: String,
        val codecs: List<String>,
        val hdrCapture: Boolean,
        val hdrRender: Boolean,
        val maxStreams: Int,
        val updatedAtMs: Long
    ) : ProtocolFrame {
        override val kind: String = "deviceCapability"
    }

    data class MediaProfileNegotiation(
        val participantId: String,
        val preferredProfile: MediaProfile,
        val negotiatedProfile: MediaProfile,
        val colorPrimaries: String,
        val transferFunction: String,
        val codec: String,
        val epoch: Int
    ) : ProtocolFrame {
        override val kind: String = "mediaProfileNegotiation"
    }

    data class RecordingNotice(
        val participantId: String,
        val state: RecordingState,
        val mode: String,
        val policyBasis: String,
        val issuedAtMs: Long,
        val issuedBy: String
    ) : ProtocolFrame {
        override val kind: String = "recordingNotice"
    }

    data class E2eeKeyEpoch(
        val participantId: String,
        val epoch: Int,
        val publicKey: String,
        val signature: String,
        val issuedAtMs: Long
    ) : ProtocolFrame {
        override val kind: String = "e2eeKeyEpoch"
    }

    data class KeyRotationAck(
        val participantId: String,
        val ackEpoch: Int,
        val receivedAtMs: Long
    ) : ProtocolFrame {
        override val kind: String = "keyRotationAck"
    }

    data class PaymentPolicy(
        val required: Boolean,
        val destinationAccount: String?
    ) : ProtocolFrame {
        override val kind: String = "paymentPolicy"
    }

    data class PaymentSettlement(
        val status: PaymentSettlementStatus
    ) : ProtocolFrame {
        override val kind: String = "paymentSettlement"
    }

    data class Ping(val sentAtMs: Long) : ProtocolFrame {
        override val kind: String = "ping"
    }

    data class Pong(val sentAtMs: Long) : ProtocolFrame {
        override val kind: String = "pong"
    }

    data class Error(
        val category: SessionErrorCategory,
        val code: String,
        val message: String
    ) : ProtocolFrame {
        override val kind: String = "error"
    }
}

object ProtocolFrameCodec {
    private val json = Json {
        ignoreUnknownKeys = true
        explicitNulls = false
    }

    fun encode(frame: ProtocolFrame): String {
        return when (frame) {
            is ProtocolFrame.Handshake -> buildJsonObject {
                put("kind", JsonPrimitive("handshake"))
                put("handshake", buildJsonObject {
                    put("roomID", JsonPrimitive(frame.roomId))
                    put("participantID", JsonPrimitive(frame.participantId))
                    put("participantName", JsonPrimitive(frame.participantName))
                    frame.walletIdentity?.let { put("walletIdentity", JsonPrimitive(it)) }
                    frame.resumeToken?.let { put("resumeToken", JsonPrimitive(it)) }
                    put("preferredProfile", JsonPrimitive(frame.preferredProfile.toProtocolValue()))
                    put("hdrCapture", JsonPrimitive(frame.hdrCapture))
                    put("hdrRender", JsonPrimitive(frame.hdrRender))
                    put("sentAtMs", JsonPrimitive(frame.sentAtMs))
                })
            }

            is ProtocolFrame.HandshakeAck -> buildJsonObject {
                put("kind", JsonPrimitive("handshakeAck"))
                put("handshakeAck", buildJsonObject {
                    put("sessionID", JsonPrimitive(frame.sessionId))
                    put("resumeToken", JsonPrimitive(frame.resumeToken))
                    put("acceptedAtMs", JsonPrimitive(frame.acceptedAtMs))
                })
            }

            is ProtocolFrame.PresenceDelta -> buildJsonObject {
                put("kind", JsonPrimitive("participantPresenceDelta"))
                put("presenceDelta", buildJsonObject {
                    put("joined", buildJsonArray {
                        frame.joined.forEach { add(it.toJson()) }
                    })
                    put("left", buildJsonArray {
                        frame.left.forEach { add(JsonPrimitive(it)) }
                    })
                    put("roleChanges", buildJsonArray {
                        frame.roleChanges.forEach { add(it.toJson()) }
                    })
                    put("sequence", JsonPrimitive(frame.sequence))
                })
            }

            is ProtocolFrame.RoleGrant -> buildJsonObject {
                put("kind", JsonPrimitive("roleGrant"))
                put("roleGrant", buildJsonObject {
                    put("targetParticipantID", JsonPrimitive(frame.targetParticipantId))
                    put("role", JsonPrimitive(frame.role.toProtocolValue()))
                    put("grantedBy", JsonPrimitive(frame.grantedBy))
                    put("signature", JsonPrimitive(frame.signature))
                    put("issuedAtMs", JsonPrimitive(frame.issuedAtMs))
                })
            }

            is ProtocolFrame.RoleRevoke -> buildJsonObject {
                put("kind", JsonPrimitive("roleRevoke"))
                put("roleRevoke", buildJsonObject {
                    put("targetParticipantID", JsonPrimitive(frame.targetParticipantId))
                    put("role", JsonPrimitive(frame.role.toProtocolValue()))
                    put("revokedBy", JsonPrimitive(frame.revokedBy))
                    put("signature", JsonPrimitive(frame.signature))
                    put("issuedAtMs", JsonPrimitive(frame.issuedAtMs))
                })
            }

            is ProtocolFrame.PermissionsSnapshot -> buildJsonObject {
                put("kind", JsonPrimitive("permissionsSnapshot"))
                put("permissionsSnapshot", buildJsonObject {
                    put("participantID", JsonPrimitive(frame.participantId))
                    put("effectivePermissions", buildJsonArray {
                        frame.effectivePermissions.forEach { add(JsonPrimitive(it)) }
                    })
                    put("epoch", JsonPrimitive(frame.epoch))
                })
            }

            is ProtocolFrame.ModerationSigned -> buildJsonObject {
                put("kind", JsonPrimitive("moderationSigned"))
                put("moderationSigned", buildJsonObject {
                    put("sentAtMs", JsonPrimitive(frame.sentAtMs))
                    put("targetParticipantID", JsonPrimitive(frame.targetParticipantId))
                    put("action", JsonPrimitive(frame.action.toProtocolValue()))
                    put("issuedBy", JsonPrimitive(frame.issuedBy))
                    put("signature", JsonPrimitive(frame.signature))
                })
            }

            is ProtocolFrame.SessionPolicy -> buildJsonObject {
                put("kind", JsonPrimitive("sessionPolicy"))
                put("sessionPolicy", buildJsonObject {
                    put("roomLock", JsonPrimitive(frame.roomLock))
                    put("waitingRoomEnabled", JsonPrimitive(frame.waitingRoomEnabled))
                    put("recordingPolicy", JsonPrimitive(frame.recordingPolicy.toProtocolValue()))
                    put("guestPolicy", JsonPrimitive(frame.guestPolicy.toProtocolValue()))
                    put("e2eeRequired", JsonPrimitive(frame.e2eeRequired))
                    put("maxParticipants", JsonPrimitive(frame.maxParticipants))
                    put("policyEpoch", JsonPrimitive(frame.policyEpoch))
                    put("updatedBy", JsonPrimitive(frame.updatedBy))
                    put("signature", JsonPrimitive(frame.signature))
                })
            }

            is ProtocolFrame.DeviceCapability -> buildJsonObject {
                put("kind", JsonPrimitive("deviceCapability"))
                put("deviceCapability", buildJsonObject {
                    put("participantID", JsonPrimitive(frame.participantId))
                    put("codecs", buildJsonArray {
                        frame.codecs.forEach { add(JsonPrimitive(it)) }
                    })
                    put("hdrCapture", JsonPrimitive(frame.hdrCapture))
                    put("hdrRender", JsonPrimitive(frame.hdrRender))
                    put("maxStreams", JsonPrimitive(frame.maxStreams))
                    put("updatedAtMs", JsonPrimitive(frame.updatedAtMs))
                })
            }

            is ProtocolFrame.MediaProfileNegotiation -> buildJsonObject {
                put("kind", JsonPrimitive("mediaProfileNegotiation"))
                put("mediaProfileNegotiation", buildJsonObject {
                    put("participantID", JsonPrimitive(frame.participantId))
                    put("preferredProfile", JsonPrimitive(frame.preferredProfile.toProtocolValue()))
                    put("negotiatedProfile", JsonPrimitive(frame.negotiatedProfile.toProtocolValue()))
                    put("colorPrimaries", JsonPrimitive(frame.colorPrimaries))
                    put("transferFunction", JsonPrimitive(frame.transferFunction))
                    put("codec", JsonPrimitive(frame.codec))
                    put("epoch", JsonPrimitive(frame.epoch))
                })
            }

            is ProtocolFrame.RecordingNotice -> buildJsonObject {
                put("kind", JsonPrimitive("recordingNotice"))
                put("recordingNotice", buildJsonObject {
                    put("participantID", JsonPrimitive(frame.participantId))
                    put("state", JsonPrimitive(frame.state.toProtocolValue()))
                    put("mode", JsonPrimitive(frame.mode))
                    put("policyBasis", JsonPrimitive(frame.policyBasis))
                    put("issuedAtMs", JsonPrimitive(frame.issuedAtMs))
                    put("issuedBy", JsonPrimitive(frame.issuedBy))
                })
            }

            is ProtocolFrame.E2eeKeyEpoch -> buildJsonObject {
                put("kind", JsonPrimitive("e2eeKeyEpoch"))
                put("e2eeKeyEpoch", buildJsonObject {
                    put("participantID", JsonPrimitive(frame.participantId))
                    put("epoch", JsonPrimitive(frame.epoch))
                    put("publicKey", JsonPrimitive(frame.publicKey))
                    put("signature", JsonPrimitive(frame.signature))
                    put("issuedAtMs", JsonPrimitive(frame.issuedAtMs))
                })
            }

            is ProtocolFrame.KeyRotationAck -> buildJsonObject {
                put("kind", JsonPrimitive("keyRotationAck"))
                put("keyRotationAck", buildJsonObject {
                    put("participantID", JsonPrimitive(frame.participantId))
                    put("ackEpoch", JsonPrimitive(frame.ackEpoch))
                    put("receivedAtMs", JsonPrimitive(frame.receivedAtMs))
                })
            }

            is ProtocolFrame.PaymentPolicy -> buildJsonObject {
                put("kind", JsonPrimitive("paymentPolicy"))
                put("paymentPolicy", buildJsonObject {
                    put("required", JsonPrimitive(frame.required))
                    frame.destinationAccount?.let { put("destinationAccount", JsonPrimitive(it)) }
                })
            }

            is ProtocolFrame.PaymentSettlement -> buildJsonObject {
                put("kind", JsonPrimitive("paymentSettlement"))
                put("paymentSettlement", buildJsonObject {
                    put("status", JsonPrimitive(frame.status.toProtocolValue()))
                })
            }

            is ProtocolFrame.Ping -> buildJsonObject {
                put("kind", JsonPrimitive("ping"))
                put("ping", buildJsonObject {
                    put("sentAtMs", JsonPrimitive(frame.sentAtMs))
                })
            }

            is ProtocolFrame.Pong -> buildJsonObject {
                put("kind", JsonPrimitive("pong"))
                put("pong", buildJsonObject {
                    put("sentAtMs", JsonPrimitive(frame.sentAtMs))
                })
            }

            is ProtocolFrame.Error -> buildJsonObject {
                put("kind", JsonPrimitive("error"))
                put("error", buildJsonObject {
                    put("category", JsonPrimitive(frame.category.toProtocolValue()))
                    put("code", JsonPrimitive(frame.code))
                    put("message", JsonPrimitive(frame.message))
                })
            }
        }.toString()
    }

    fun decode(raw: String): ProtocolFrame? {
        val root = runCatching { json.parseToJsonElement(raw).jsonObject }.getOrNull() ?: return null
        val kind = root.stringOrNull("kind") ?: return null

        return when (kind) {
            "handshake" -> decodeHandshake(root)
            "handshakeAck", "handshake_ack" -> decodeHandshakeAck(root)
            "participantPresenceDelta", "participant_presence_delta" -> decodePresenceDelta(root)
            "roleGrant", "role_grant" -> decodeRoleGrant(root)
            "roleRevoke", "role_revoke" -> decodeRoleRevoke(root)
            "permissionsSnapshot", "permissions_snapshot" -> decodePermissionsSnapshot(root)
            "moderationSigned", "moderation_signed" -> decodeModerationSigned(root)
            "sessionPolicy", "session_policy" -> decodeSessionPolicy(root)
            "deviceCapability", "device_capability" -> decodeDeviceCapability(root)
            "mediaProfileNegotiation", "media_profile_negotiation" -> decodeMediaProfileNegotiation(root)
            "recordingNotice", "recording_notice" -> decodeRecordingNotice(root)
            "e2eeKeyEpoch", "e2ee_key_epoch" -> decodeE2eeKeyEpoch(root)
            "keyRotationAck", "key_rotation_ack" -> decodeKeyRotationAck(root)
            "paymentPolicy", "payment_policy" -> decodePaymentPolicy(root)
            "paymentSettlement", "payment_settlement" -> decodePaymentSettlement(root)
            "ping" -> decodePing(root)
            "pong" -> decodePong(root)
            "error" -> decodeError(root)
            else -> null
        }
    }

    private fun decodeHandshake(root: JsonObject): ProtocolFrame? {
        val payload = root.objectOrNull("handshake") ?: root
        return ProtocolFrame.Handshake(
            roomId = payload.stringOrNull("roomID", "roomId", "room_id") ?: return null,
            participantId = payload.stringOrNull("participantID", "participantId", "participant_id") ?: return null,
            participantName = payload.stringOrNull("participantName", "participant_name") ?: return null,
            walletIdentity = payload.stringOrNull("walletIdentity", "wallet_identity"),
            resumeToken = payload.stringOrNull("resumeToken", "resume_token"),
            preferredProfile = MediaProfile.fromWire(payload.stringOrNull("preferredProfile", "preferred_profile")),
            hdrCapture = payload.booleanOrNull("hdrCapture", "hdr_capture") ?: false,
            hdrRender = payload.booleanOrNull("hdrRender", "hdr_render") ?: false,
            sentAtMs = payload.longOrNull("sentAtMs", "sent_at_ms") ?: 0L
        )
    }

    private fun decodeHandshakeAck(root: JsonObject): ProtocolFrame? {
        val payload = root.objectOrNull("handshakeAck", "handshake_ack") ?: root
        return ProtocolFrame.HandshakeAck(
            sessionId = payload.stringOrNull("sessionID", "sessionId", "session_id") ?: return null,
            resumeToken = payload.stringOrNull("resumeToken", "resume_token") ?: return null,
            acceptedAtMs = payload.longOrNull("acceptedAtMs", "accepted_at_ms") ?: 0L
        )
    }

    private fun decodePresenceDelta(root: JsonObject): ProtocolFrame? {
        val payload = root.objectOrNull("presenceDelta", "participantPresenceDelta", "participant_presence_delta") ?: root
        return ProtocolFrame.PresenceDelta(
            joined = payload.arrayOrEmpty("joined").mapNotNull { it.asParticipantOrNull() },
            left = payload.arrayOrEmpty("left").mapNotNull { it.stringOrNull() },
            roleChanges = payload.arrayOrEmpty("roleChanges", "role_changes").mapNotNull { it.asRoleChangeOrNull() },
            sequence = payload.longOrNull("sequence") ?: 0L
        )
    }

    private fun decodeRoleGrant(root: JsonObject): ProtocolFrame? {
        val payload = root.objectOrNull("roleGrant", "role_grant") ?: root
        return ProtocolFrame.RoleGrant(
            targetParticipantId = payload.stringOrNull("targetParticipantID", "targetParticipantId", "target_participant_id")
                ?: return null,
            role = ParticipantRole.fromWire(payload.stringOrNull("role")),
            grantedBy = payload.stringOrNull("grantedBy", "granted_by") ?: "unknown",
            signature = payload.stringOrNull("signature") ?: "",
            issuedAtMs = payload.longOrNull("issuedAtMs", "issued_at_ms") ?: 0L
        )
    }

    private fun decodeRoleRevoke(root: JsonObject): ProtocolFrame? {
        val payload = root.objectOrNull("roleRevoke", "role_revoke") ?: root
        return ProtocolFrame.RoleRevoke(
            targetParticipantId = payload.stringOrNull("targetParticipantID", "targetParticipantId", "target_participant_id")
                ?: return null,
            role = ParticipantRole.fromWire(payload.stringOrNull("role")),
            revokedBy = payload.stringOrNull("revokedBy", "revoked_by") ?: "unknown",
            signature = payload.stringOrNull("signature") ?: "",
            issuedAtMs = payload.longOrNull("issuedAtMs", "issued_at_ms") ?: 0L
        )
    }

    private fun decodePermissionsSnapshot(root: JsonObject): ProtocolFrame? {
        val payload = root.objectOrNull("permissionsSnapshot", "permissions_snapshot") ?: root
        return ProtocolFrame.PermissionsSnapshot(
            participantId = payload.stringOrNull("participantID", "participantId", "participant_id") ?: return null,
            effectivePermissions = payload.arrayOrEmpty("effectivePermissions", "effective_permissions")
                .mapNotNull { it.stringOrNull() },
            epoch = payload.intOrNull("epoch") ?: 0
        )
    }

    private fun decodeModerationSigned(root: JsonObject): ProtocolFrame? {
        val payload = root.objectOrNull("moderationSigned", "moderation_signed") ?: root
        return ProtocolFrame.ModerationSigned(
            sentAtMs = payload.longOrNull("sentAtMs", "sent_at_ms") ?: 0L,
            targetParticipantId = payload.stringOrNull("targetParticipantID", "targetParticipantId", "target_participant_id")
                ?: return null,
            action = ModerationAction.fromWire(payload.stringOrNull("action")),
            issuedBy = payload.stringOrNull("issuedBy", "issued_by") ?: "unknown",
            signature = payload.stringOrNull("signature") ?: ""
        )
    }

    private fun decodeSessionPolicy(root: JsonObject): ProtocolFrame? {
        val payload = root.objectOrNull("sessionPolicy", "session_policy") ?: root
        return ProtocolFrame.SessionPolicy(
            roomLock = payload.booleanOrNull("roomLock", "room_lock") ?: false,
            waitingRoomEnabled = payload.booleanOrNull("waitingRoomEnabled", "waiting_room_enabled") ?: false,
            recordingPolicy = RecordingState.fromWire(payload.stringOrNull("recordingPolicy", "recording_policy")),
            guestPolicy = GuestPolicy.fromWire(payload.stringOrNull("guestPolicy", "guest_policy")),
            e2eeRequired = payload.booleanOrNull("e2eeRequired", "e2ee_required") ?: true,
            maxParticipants = payload.intOrNull("maxParticipants", "max_participants") ?: 500,
            policyEpoch = payload.intOrNull("policyEpoch", "policy_epoch") ?: 0,
            updatedBy = payload.stringOrNull("updatedBy", "updated_by") ?: "system",
            signature = payload.stringOrNull("signature") ?: ""
        )
    }

    private fun decodeDeviceCapability(root: JsonObject): ProtocolFrame? {
        val payload = root.objectOrNull("deviceCapability", "device_capability") ?: root
        return ProtocolFrame.DeviceCapability(
            participantId = payload.stringOrNull("participantID", "participantId", "participant_id") ?: return null,
            codecs = payload.arrayOrEmpty("codecs").mapNotNull { it.stringOrNull() },
            hdrCapture = payload.booleanOrNull("hdrCapture", "hdr_capture") ?: false,
            hdrRender = payload.booleanOrNull("hdrRender", "hdr_render") ?: false,
            maxStreams = payload.intOrNull("maxStreams", "max_streams") ?: 1,
            updatedAtMs = payload.longOrNull("updatedAtMs", "updated_at_ms") ?: 0L
        )
    }

    private fun decodeMediaProfileNegotiation(root: JsonObject): ProtocolFrame? {
        val payload = root.objectOrNull("mediaProfileNegotiation", "media_profile_negotiation") ?: root
        return ProtocolFrame.MediaProfileNegotiation(
            participantId = payload.stringOrNull("participantID", "participantId", "participant_id") ?: return null,
            preferredProfile = MediaProfile.fromWire(payload.stringOrNull("preferredProfile", "preferred_profile")),
            negotiatedProfile = MediaProfile.fromWire(payload.stringOrNull("negotiatedProfile", "negotiated_profile")),
            colorPrimaries = payload.stringOrNull("colorPrimaries", "color_primaries") ?: "bt709",
            transferFunction = payload.stringOrNull("transferFunction", "transfer_function") ?: "gamma",
            codec = payload.stringOrNull("codec") ?: "h264",
            epoch = payload.intOrNull("epoch") ?: 0
        )
    }

    private fun decodeRecordingNotice(root: JsonObject): ProtocolFrame? {
        val payload = root.objectOrNull("recordingNotice", "recording_notice") ?: root
        return ProtocolFrame.RecordingNotice(
            participantId = payload.stringOrNull("participantID", "participantId", "participant_id") ?: return null,
            state = RecordingState.fromWire(payload.stringOrNull("state")),
            mode = payload.stringOrNull("mode") ?: "local",
            policyBasis = payload.stringOrNull("policyBasis", "policy_basis") ?: "policy-default",
            issuedAtMs = payload.longOrNull("issuedAtMs", "issued_at_ms") ?: 0L,
            issuedBy = payload.stringOrNull("issuedBy", "issued_by") ?: "system"
        )
    }

    private fun decodeE2eeKeyEpoch(root: JsonObject): ProtocolFrame? {
        val payload = root.objectOrNull("e2eeKeyEpoch", "e2ee_key_epoch") ?: root
        return ProtocolFrame.E2eeKeyEpoch(
            participantId = payload.stringOrNull("participantID", "participantId", "participant_id") ?: return null,
            epoch = payload.intOrNull("epoch") ?: 0,
            publicKey = payload.stringOrNull("publicKey", "public_key") ?: "",
            signature = payload.stringOrNull("signature") ?: "",
            issuedAtMs = payload.longOrNull("issuedAtMs", "issued_at_ms") ?: 0L
        )
    }

    private fun decodeKeyRotationAck(root: JsonObject): ProtocolFrame? {
        val payload = root.objectOrNull("keyRotationAck", "key_rotation_ack") ?: root
        return ProtocolFrame.KeyRotationAck(
            participantId = payload.stringOrNull("participantID", "participantId", "participant_id") ?: return null,
            ackEpoch = payload.intOrNull("ackEpoch", "ack_epoch") ?: 0,
            receivedAtMs = payload.longOrNull("receivedAtMs", "received_at_ms") ?: 0L
        )
    }

    private fun decodePaymentPolicy(root: JsonObject): ProtocolFrame? {
        val payload = root.objectOrNull("paymentPolicy", "payment_policy") ?: root
        return ProtocolFrame.PaymentPolicy(
            required = payload.booleanOrNull("required") ?: false,
            destinationAccount = payload.stringOrNull("destinationAccount", "destination_account")
        )
    }

    private fun decodePaymentSettlement(root: JsonObject): ProtocolFrame? {
        val payload = root.objectOrNull("paymentSettlement", "payment_settlement") ?: root
        return ProtocolFrame.PaymentSettlement(
            status = PaymentSettlementStatus.fromWire(payload.stringOrNull("status"))
        )
    }

    private fun decodePing(root: JsonObject): ProtocolFrame {
        val payload = root.objectOrNull("ping") ?: root
        return ProtocolFrame.Ping(
            sentAtMs = payload.longOrNull("sentAtMs", "sent_at_ms") ?: 0L
        )
    }

    private fun decodePong(root: JsonObject): ProtocolFrame {
        val payload = root.objectOrNull("pong") ?: root
        return ProtocolFrame.Pong(
            sentAtMs = payload.longOrNull("sentAtMs", "sent_at_ms") ?: 0L
        )
    }

    private fun decodeError(root: JsonObject): ProtocolFrame {
        val payload = root.objectOrNull("error") ?: root
        return ProtocolFrame.Error(
            category = SessionErrorCategory.fromWire(payload.stringOrNull("category")),
            code = payload.stringOrNull("code") ?: "error",
            message = payload.stringOrNull("message") ?: "unknown"
        )
    }

    private fun Participant.toJson(): JsonObject = buildJsonObject {
        put("id", JsonPrimitive(id))
        put("displayName", JsonPrimitive(displayName))
        put("role", JsonPrimitive(role.toProtocolValue()))
        put("muted", JsonPrimitive(muted))
        put("videoEnabled", JsonPrimitive(videoEnabled))
        put("shareEnabled", JsonPrimitive(shareEnabled))
        put("waitingRoom", JsonPrimitive(waitingRoom))
    }

    private fun RoleChange.toJson(): JsonObject = buildJsonObject {
        put("participantID", JsonPrimitive(participantId))
        put("role", JsonPrimitive(role.toProtocolValue()))
    }

    private fun JsonElement.asParticipantOrNull(): Participant? {
        val obj = runCatching { jsonObject }.getOrNull() ?: return null
        return Participant(
            id = obj.stringOrNull("id") ?: return null,
            displayName = obj.stringOrNull("displayName", "display_name") ?: return null,
            role = ParticipantRole.fromWire(obj.stringOrNull("role")),
            muted = obj.booleanOrNull("muted") ?: false,
            videoEnabled = obj.booleanOrNull("videoEnabled", "video_enabled") ?: true,
            shareEnabled = obj.booleanOrNull("shareEnabled", "share_enabled") ?: false,
            waitingRoom = obj.booleanOrNull("waitingRoom", "waiting_room") ?: false
        )
    }

    private fun JsonElement.asRoleChangeOrNull(): RoleChange? {
        val obj = runCatching { jsonObject }.getOrNull() ?: return null
        return RoleChange(
            participantId = obj.stringOrNull("participantID", "participantId", "participant_id") ?: return null,
            role = ParticipantRole.fromWire(obj.stringOrNull("role"))
        )
    }

    private fun JsonElement.stringOrNull(): String? {
        return runCatching { jsonPrimitive.contentOrNull }.getOrNull()
    }

    private fun JsonObject.objectOrNull(vararg keys: String): JsonObject? {
        for (key in keys) {
            val element = this[key] ?: continue
            val obj = runCatching { element.jsonObject }.getOrNull()
            if (obj != null) return obj
        }
        return null
    }

    private fun JsonObject.arrayOrEmpty(vararg keys: String): JsonArray {
        for (key in keys) {
            val element = this[key] ?: continue
            val array = runCatching { element.jsonArray }.getOrNull()
            if (array != null) return array
        }
        return JsonArray(emptyList())
    }

    private fun JsonObject.stringOrNull(vararg keys: String): String? {
        for (key in keys) {
            val value = this[key] ?: continue
            val parsed = runCatching { value.jsonPrimitive.contentOrNull }.getOrNull()
            if (parsed != null) return parsed
        }
        return null
    }

    private fun JsonObject.booleanOrNull(vararg keys: String): Boolean? {
        for (key in keys) {
            val value = this[key] ?: continue
            val parsed = runCatching { value.jsonPrimitive.booleanOrNull }.getOrNull()
            if (parsed != null) return parsed
        }
        return null
    }

    private fun JsonObject.intOrNull(vararg keys: String): Int? {
        for (key in keys) {
            val value = this[key] ?: continue
            val parsed = runCatching { value.jsonPrimitive.intOrNull }.getOrNull()
            if (parsed != null) return parsed
        }
        return null
    }

    private fun JsonObject.longOrNull(vararg keys: String): Long? {
        for (key in keys) {
            val value = this[key] ?: continue
            val parsed = runCatching { value.jsonPrimitive.longOrNull }.getOrNull()
            if (parsed != null) return parsed
        }
        return null
    }

    private fun SessionErrorCategory.toProtocolValue(): String {
        return when (this) {
            SessionErrorCategory.ProtocolFailure -> "protocolFailure"
            SessionErrorCategory.PolicyFailure -> "policyFailure"
            SessionErrorCategory.TransportFailure -> "transportFailure"
        }
    }

    private fun ParticipantRole.toProtocolValue(): String {
        return when (this) {
            ParticipantRole.Host -> "host"
            ParticipantRole.CoHost -> "co_host"
            ParticipantRole.Participant -> "participant"
            ParticipantRole.Guest -> "guest"
        }
    }

    private fun ModerationAction.toProtocolValue(): String {
        return when (this) {
            ModerationAction.Mute -> "mute"
            ModerationAction.VideoOff -> "videoOff"
            ModerationAction.StopShare -> "stopShare"
            ModerationAction.Kick -> "kick"
            ModerationAction.AdmitFromWaiting -> "admitFromWaiting"
            ModerationAction.DenyFromWaiting -> "denyFromWaiting"
        }
    }

    private fun MediaProfile.toProtocolValue(): String {
        return when (this) {
            MediaProfile.SDR -> "sdr"
            MediaProfile.HDR -> "hdr"
        }
    }

    private fun RecordingState.toProtocolValue(): String {
        return when (this) {
            RecordingState.Stopped -> "stopped"
            RecordingState.Started -> "started"
        }
    }

    private fun GuestPolicy.toProtocolValue(): String {
        return when (this) {
            GuestPolicy.Open -> "open"
            GuestPolicy.InviteOnly -> "invite_only"
            GuestPolicy.Blocked -> "blocked"
        }
    }

    private fun PaymentSettlementStatus.toProtocolValue(): String {
        return when (this) {
            PaymentSettlementStatus.NotRequired -> "notRequired"
            PaymentSettlementStatus.Pending -> "pending"
            PaymentSettlementStatus.Settled -> "settled"
            PaymentSettlementStatus.Blocked -> "blocked"
        }
    }
}
