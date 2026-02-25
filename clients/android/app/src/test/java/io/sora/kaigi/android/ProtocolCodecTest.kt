package io.sora.kaigi.android

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class ProtocolCodecTest {
    @Test
    fun handshakeRoundTrip() {
        val frame = ProtocolFrame.Handshake(
            roomId = "room-a",
            participantId = "alice",
            participantName = "Alice",
            walletIdentity = "nexus://wallet/alice",
            resumeToken = "resume-1",
            preferredProfile = MediaProfile.HDR,
            hdrCapture = true,
            hdrRender = true,
            sentAtMs = 100L
        )

        val encoded = ProtocolFrameCodec.encode(frame)
        val decoded = ProtocolFrameCodec.decode(encoded)

        assertTrue(encoded.contains("\"kind\":\"handshake\""))
        assertTrue(encoded.contains("\"handshake\""))
        assertTrue(encoded.contains("\"walletIdentity\":\"nexus://wallet/alice\""))
        assertEquals(frame, decoded)
    }

    @Test
    fun presenceDeltaRoundTripWithNestedPayload() {
        val frame = ProtocolFrame.PresenceDelta(
            joined = listOf(
                Participant(
                    id = "alice",
                    displayName = "Alice",
                    role = ParticipantRole.Host,
                    muted = false,
                    videoEnabled = true,
                    shareEnabled = false,
                    waitingRoom = false
                )
            ),
            left = listOf("bob"),
            roleChanges = listOf(RoleChange(participantId = "alice", role = ParticipantRole.CoHost)),
            sequence = 42L
        )

        val encoded = ProtocolFrameCodec.encode(frame)
        val decoded = ProtocolFrameCodec.decode(encoded)

        assertTrue(encoded.contains("\"kind\":\"participantPresenceDelta\""))
        assertTrue(encoded.contains("\"presenceDelta\""))
        assertEquals(frame, decoded)
    }

    @Test
    fun decodesAppleStyleHandshakeAckJson() {
        val raw = """
            {"kind":"handshakeAck","handshakeAck":{"sessionID":"s1","resumeToken":"resume-2","acceptedAtMs":1700000000000}}
        """.trimIndent()

        val decoded = ProtocolFrameCodec.decode(raw)
        assertNotNull(decoded)
        val ack = decoded as ProtocolFrame.HandshakeAck
        assertEquals("s1", ack.sessionId)
        assertEquals("resume-2", ack.resumeToken)
        assertEquals(1700000000000L, ack.acceptedAtMs)
    }

    @Test
    fun decodesSnakeCaseHandshakeAckJson() {
        val raw = """
            {"kind":"handshake_ack","handshake_ack":{"session_id":"s2","resume_token":"resume-3","accepted_at_ms":1700000000001}}
        """.trimIndent()

        val decoded = ProtocolFrameCodec.decode(raw)
        val ack = decoded as ProtocolFrame.HandshakeAck
        assertEquals("s2", ack.sessionId)
        assertEquals("resume-3", ack.resumeToken)
        assertEquals(1700000000001L, ack.acceptedAtMs)
    }

    @Test
    fun moderationRoundTripIncludesSignature() {
        val frame = ProtocolFrame.ModerationSigned(
            sentAtMs = 10L,
            targetParticipantId = "target",
            action = ModerationAction.Mute,
            issuedBy = "host",
            signature = "sig-123"
        )

        val encoded = ProtocolFrameCodec.encode(frame)
        val decoded = ProtocolFrameCodec.decode(encoded)

        assertTrue(encoded.contains("\"signature\":\"sig-123\""))
        assertEquals(frame, decoded)
    }

    @Test
    fun decodesSessionPolicyWithGuestPolicyAndSignature() {
        val raw = """
            {"kind":"sessionPolicy","sessionPolicy":{"roomLock":true,"waitingRoomEnabled":true,"recordingPolicy":"started","guestPolicy":"invite_only","e2eeRequired":true,"maxParticipants":500,"policyEpoch":3,"updatedBy":"host","signature":"sig-policy"}}
        """.trimIndent()

        val decoded = ProtocolFrameCodec.decode(raw)
        assertNotNull(decoded)
        val policy = decoded as ProtocolFrame.SessionPolicy
        assertEquals(GuestPolicy.InviteOnly, policy.guestPolicy)
        assertEquals("sig-policy", policy.signature)
    }

    @Test
    fun decodesPermissionsSnapshotPayload() {
        val raw = """
            {"kind":"permissionsSnapshot","permissionsSnapshot":{"participantID":"alice","effectivePermissions":["moderate","share"],"epoch":4}}
        """.trimIndent()

        val decoded = ProtocolFrameCodec.decode(raw)
        assertNotNull(decoded)
        val snapshot = decoded as ProtocolFrame.PermissionsSnapshot
        assertEquals("alice", snapshot.participantId)
        assertEquals(listOf("moderate", "share"), snapshot.effectivePermissions)
        assertEquals(4, snapshot.epoch)
    }

    @Test
    fun decodesSnakeCasePermissionsSnapshotPayload() {
        val raw = """
            {"kind":"permissions_snapshot","permissions_snapshot":{"participant_id":"alice","effective_permissions":["moderate","share"],"epoch":5}}
        """.trimIndent()

        val decoded = ProtocolFrameCodec.decode(raw)
        assertNotNull(decoded)
        val snapshot = decoded as ProtocolFrame.PermissionsSnapshot
        assertEquals("alice", snapshot.participantId)
        assertEquals(listOf("moderate", "share"), snapshot.effectivePermissions)
        assertEquals(5, snapshot.epoch)
    }

    @Test
    fun decodesPresenceDeltaAliases() {
        val raw = """
            {"kind":"participant_presence_delta","participant_presence_delta":{"joined":[{"id":"alice","display_name":"Alice","role":"coHost","muted":false,"video_enabled":true,"share_enabled":true,"waiting_room":true}],"left":["bob"],"role_changes":[{"participant_id":"alice","role":"co_host"}],"sequence":9}}
        """.trimIndent()

        val decoded = ProtocolFrameCodec.decode(raw)
        val delta = decoded as ProtocolFrame.PresenceDelta
        assertEquals(9L, delta.sequence)
        assertEquals("Alice", delta.joined.first().displayName)
        assertEquals(ParticipantRole.CoHost, delta.joined.first().role)
        assertEquals(true, delta.joined.first().waitingRoom)
        assertEquals("alice", delta.roleChanges.first().participantId)
        assertEquals(ParticipantRole.CoHost, delta.roleChanges.first().role)
    }

    @Test
    fun decodesSessionPolicyAliases() {
        val raw = """
            {"kind":"session_policy","session_policy":{"room_lock":true,"waiting_room_enabled":true,"recording_policy":"started","guest_policy":"inviteOnly","e2ee_required":true,"max_participants":300,"policy_epoch":8,"updated_by":"host","signature":"sig-policy"}}
        """.trimIndent()

        val decoded = ProtocolFrameCodec.decode(raw)
        val policy = decoded as ProtocolFrame.SessionPolicy
        assertEquals(true, policy.roomLock)
        assertEquals(true, policy.waitingRoomEnabled)
        assertEquals(RecordingState.Started, policy.recordingPolicy)
        assertEquals(GuestPolicy.InviteOnly, policy.guestPolicy)
        assertEquals(300, policy.maxParticipants)
        assertEquals(8, policy.policyEpoch)
        assertEquals("host", policy.updatedBy)
        assertEquals("sig-policy", policy.signature)
    }

    @Test
    fun decodesPolicyFailureErrorAlias() {
        val raw = """
            {"kind":"error","error":{"category":"policy_failure","code":"policy_reject","message":"blocked"}}
        """.trimIndent()

        val decoded = ProtocolFrameCodec.decode(raw)
        val error = decoded as ProtocolFrame.Error
        assertEquals(SessionErrorCategory.PolicyFailure, error.category)
        assertEquals("policy_reject", error.code)
    }

    @Test
    fun decodesNotRequiredPaymentSettlementAlias() {
        val raw = """
            {"kind":"payment_settlement","payment_settlement":{"status":"not_required"}}
        """.trimIndent()

        val decoded = ProtocolFrameCodec.decode(raw)
        val settlement = decoded as ProtocolFrame.PaymentSettlement
        assertEquals(PaymentSettlementStatus.NotRequired, settlement.status)
    }

    @Test
    fun rejectsLegacyRawJoinFrame() {
        val decoded = ProtocolFrameCodec.decode("JOIN room=daily participant=Alice")
        assertNull(decoded)
    }
}
