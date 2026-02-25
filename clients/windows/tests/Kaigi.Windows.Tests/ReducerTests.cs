using Kaigi.Windows;

namespace Kaigi.Windows.Tests;

public sealed class InMemoryMeetingTelemetrySink : IMeetingTelemetrySink
{
    private readonly List<MeetingTelemetryEvent> events = new();

    public IReadOnlyList<MeetingTelemetryEvent> Events => events;

    public void Record(MeetingTelemetryEvent telemetryEvent)
    {
        events.Add(telemetryEvent);
    }
}

public sealed class ReducerTests
{
    [Fact]
    public void HandshakeAckMarksConnected()
    {
        var state = ProtocolSessionState.Initial(new MeetingConfig());
        var next = ProtocolReducer.Reduce(
            state,
            new ProtocolEvent.FrameReceived(new ProtocolFrame.HandshakeAck("session", "token", 10)),
            nowMs: 10);

        Assert.Equal(ConnectionPhase.Connected, next.ConnectionPhase);
        Assert.True(next.HandshakeComplete);
        Assert.Equal("token", next.ResumeToken);
    }

    [Fact]
    public void PresenceDeltaMonotonicityPreventsStaleRollback()
    {
        var state = ProtocolSessionState.Initial(new MeetingConfig());
        var joined = new Participant(
            Id: "p2",
            DisplayName: "Beta",
            Role: ParticipantRole.Participant,
            Muted: false,
            VideoEnabled: true,
            ShareEnabled: true,
            WaitingRoom: false);

        var next = ProtocolReducer.Reduce(
            state,
            new ProtocolEvent.FrameReceived(new ProtocolFrame.PresenceDelta(
                Joined: new[] { joined },
                Left: Array.Empty<string>(),
                RoleChanges: Array.Empty<RoleChange>(),
                Sequence: 4)),
            nowMs: 1);

        var stale = ProtocolReducer.Reduce(
            next,
            new ProtocolEvent.FrameReceived(new ProtocolFrame.PresenceDelta(
                Joined: Array.Empty<Participant>(),
                Left: new[] { "p2" },
                RoleChanges: Array.Empty<RoleChange>(),
                Sequence: 3)),
            nowMs: 2);

        Assert.Equal(4, stale.PresenceSequence);
        Assert.True(stale.Participants.ContainsKey("p2"));
    }

    [Fact]
    public void UnsignedRoleGrantRejectedWhenPolicyRequiresSignatures()
    {
        var config = new MeetingConfig
        {
            RequireSignedModeration = true,
            PreferWebFallbackOnPolicyFailure = false
        };
        var state = ProtocolSessionState.Initial(config);
        state = state with
        {
            Participants = new Dictionary<string, Participant>
            {
                ["host"] = new Participant("host", "Host", ParticipantRole.Host, false, true, true, false)
            }
        };

        var next = ProtocolReducer.Reduce(
            state,
            new ProtocolEvent.FrameReceived(new ProtocolFrame.RoleGrant(
                TargetParticipantId: "host",
                Role: ParticipantRole.CoHost,
                GrantedBy: "host",
                Signature: null,
                IssuedAtMs: 11)),
            nowMs: 11);

        Assert.Equal(ConnectionPhase.Error, next.ConnectionPhase);
        Assert.Equal("role_grant_signature_missing", next.LastError?.Code);
    }

    [Fact]
    public void SessionPolicyRequiresE2eeEpochThenClearsAfterEpochAndAck()
    {
        var config = new MeetingConfig
        {
            PreferWebFallbackOnPolicyFailure = false
        };
        var state = ProtocolSessionState.Initial(config);
        state = state with
        {
            Participants = new Dictionary<string, Participant>
            {
                ["host"] = new Participant("host", "Host", ParticipantRole.Host, false, true, true, false)
            }
        };

        var connected = ProtocolReducer.Reduce(
            state,
            new ProtocolEvent.FrameReceived(new ProtocolFrame.HandshakeAck("s1", "r1", 1)),
            nowMs: 1);

        var rejected = ProtocolReducer.Reduce(
            connected,
            new ProtocolEvent.FrameReceived(new ProtocolFrame.SessionPolicy(
                RoomLock: true,
                WaitingRoomEnabled: true,
                RecordingPolicy: RecordingState.Started,
                GuestPolicy: GuestPolicy.InviteOnly,
                E2eeRequired: true,
                MaxParticipants: 500,
                PolicyEpoch: 9,
                UpdatedBy: "host",
                Signature: "sig-policy",
                UpdatedAtMs: 10)),
            nowMs: 10);

        Assert.Equal(ConnectionPhase.Error, rejected.ConnectionPhase);
        Assert.Equal("e2ee_epoch_required", rejected.LastError?.Code);
        Assert.Equal(RecordingState.Started, rejected.RecordingNotice);

        var epochUpdated = ProtocolReducer.Reduce(
            rejected,
            new ProtocolEvent.FrameReceived(new ProtocolFrame.E2eeKeyEpoch(
                Epoch: 7,
                IssuedBy: "host",
                Signature: "sig-e2ee",
                SentAtMs: 11)),
            nowMs: 11);

        var ackUpdated = ProtocolReducer.Reduce(
            epochUpdated,
            new ProtocolEvent.FrameReceived(new ProtocolFrame.KeyRotationAck(
                ParticipantId: "host",
                AckEpoch: 7,
                ReceivedAtMs: 12)),
            nowMs: 12);

        Assert.Equal(ConnectionPhase.Connected, ackUpdated.ConnectionPhase);
        Assert.Null(ackUpdated.LastError);
        Assert.Equal(7, ackUpdated.E2eeState.CurrentEpoch);
        Assert.Equal(7, ackUpdated.E2eeState.LastAckEpoch);
    }

    [Fact]
    public void PaymentPolicyRejectsWhenUnsettledThenClearsWhenSettled()
    {
        var config = new MeetingConfig
        {
            RequirePaymentSettlement = true,
            PreferWebFallbackOnPolicyFailure = false
        };
        var state = ProtocolSessionState.Initial(config);

        var connected = ProtocolReducer.Reduce(
            state,
            new ProtocolEvent.FrameReceived(new ProtocolFrame.HandshakeAck("s2", "r2", 1)),
            nowMs: 1);

        var unsettled = ProtocolReducer.Reduce(
            connected,
            new ProtocolEvent.FrameReceived(new ProtocolFrame.PaymentPolicy(
                Required: true,
                DestinationAccount: "nexus://dest")),
            nowMs: 2);

        Assert.Equal(ConnectionPhase.Error, unsettled.ConnectionPhase);
        Assert.Equal("payment_settlement_required", unsettled.LastError?.Code);
        Assert.Equal(PaymentSettlementStatus.Pending, unsettled.PaymentState.SettlementStatus);

        var settled = ProtocolReducer.Reduce(
            unsettled,
            new ProtocolEvent.FrameReceived(new ProtocolFrame.PaymentSettlement(PaymentSettlementStatus.Settled)),
            nowMs: 3);

        Assert.Equal(ConnectionPhase.Connected, settled.ConnectionPhase);
        Assert.Null(settled.LastError);
        Assert.Equal(PaymentSettlementStatus.Settled, settled.PaymentState.SettlementStatus);
    }

    [Fact]
    public void PaymentPolicyUsesBlockedCodeWhenSettlementBlocked()
    {
        var config = new MeetingConfig
        {
            RequirePaymentSettlement = true,
            PreferWebFallbackOnPolicyFailure = false
        };
        var state = ProtocolSessionState.Initial(config);

        var connected = ProtocolReducer.Reduce(
            state,
            new ProtocolEvent.FrameReceived(new ProtocolFrame.HandshakeAck("s3", "r3", 1)),
            nowMs: 1);

        var pending = ProtocolReducer.Reduce(
            connected,
            new ProtocolEvent.FrameReceived(new ProtocolFrame.PaymentPolicy(
                Required: true,
                DestinationAccount: "nexus://dest")),
            nowMs: 2);

        var blocked = ProtocolReducer.Reduce(
            pending,
            new ProtocolEvent.FrameReceived(new ProtocolFrame.PaymentSettlement(PaymentSettlementStatus.Blocked)),
            nowMs: 3);

        Assert.Equal(ConnectionPhase.Error, blocked.ConnectionPhase);
        Assert.Equal("payment_settlement_blocked", blocked.LastError?.Code);
        Assert.Equal(PaymentSettlementStatus.Blocked, blocked.PaymentState.SettlementStatus);
    }

    [Fact]
    public void PaymentPolicyErrorClearsWhenSettlementIsNotRequired()
    {
        var config = new MeetingConfig
        {
            RequirePaymentSettlement = true,
            PreferWebFallbackOnPolicyFailure = false
        };
        var state = ProtocolSessionState.Initial(config);

        var pending = ProtocolReducer.Reduce(
            state,
            new ProtocolEvent.FrameReceived(new ProtocolFrame.PaymentPolicy(
                Required: true,
                DestinationAccount: "nexus://dest")),
            nowMs: 2);

        Assert.Equal("payment_settlement_required", pending.LastError?.Code);

        var notRequired = ProtocolReducer.Reduce(
            pending,
            new ProtocolEvent.FrameReceived(new ProtocolFrame.PaymentSettlement(PaymentSettlementStatus.NotRequired)),
            nowMs: 3);

        Assert.Equal(ConnectionPhase.Connecting, notRequired.ConnectionPhase);
        Assert.Null(notRequired.LastError);
    }

    [Fact]
    public void UnauthorizedModerationIssuerRejectedWhenSignaturesOptional()
    {
        var config = new MeetingConfig
        {
            RequireSignedModeration = false,
            PreferWebFallbackOnPolicyFailure = false
        };
        var state = ProtocolSessionState.Initial(config);
        state = state with
        {
            Participants = new Dictionary<string, Participant>
            {
                ["host"] = new Participant("host", "Host", ParticipantRole.Host, false, true, true, false),
                ["participant-1"] = new Participant("participant-1", "Participant 1", ParticipantRole.Participant, false, true, true, false)
            }
        };

        var next = ProtocolReducer.Reduce(
            state,
            new ProtocolEvent.FrameReceived(new ProtocolFrame.ModerationSigned(
                TargetParticipantId: "host",
                Action: ModerationAction.Mute,
                IssuedBy: "participant-1",
                Signature: null,
                IssuedAtMs: 11)),
            nowMs: 11);

        Assert.Equal(ConnectionPhase.Error, next.ConnectionPhase);
        Assert.Equal("moderation_not_authorized", next.LastError?.Code);
    }

    [Fact]
    public void FallbackRecoveryStoresRto()
    {
        var state = ProtocolSessionState.Initial(new MeetingConfig());
        var active = ProtocolReducer.Reduce(state, new ProtocolEvent.FallbackActivated("drill"), nowMs: 1000);
        var recovered = ProtocolReducer.Reduce(active, new ProtocolEvent.FallbackRecovered(), nowMs: 1900);

        Assert.Equal(900, recovered.Fallback.LastRtoMs);
        Assert.Equal(ConnectionPhase.Disconnected, recovered.ConnectionPhase);
    }

    [Fact]
    public void CodecDecodesHandshakeAckAlias()
    {
        const string payload = """
        {
          "kind": "handshake_ack",
          "session_id": "s1",
          "resume_token": "r2",
          "accepted_at_ms": 42
        }
        """;

        var frame = ProtocolCodec.DecodeFrame(payload);
        var ack = Assert.IsType<ProtocolFrame.HandshakeAck>(frame);
        Assert.Equal("s1", ack.SessionId);
        Assert.Equal("r2", ack.ResumeToken);
    }

    [Fact]
    public void CodecRejectsLegacyRawJoinFrame()
    {
        Assert.Throws<System.Text.Json.JsonException>(
            () => ProtocolCodec.DecodeFrame("JOIN room=daily participant=alice"));
    }

    [Fact]
    public void CodecDecodesSessionPolicyAndPaymentAliases()
    {
        const string policyPayload = """
        {
          "kind": "session_policy",
          "session_policy": {
            "room_lock": true,
            "waiting_room_enabled": true,
            "recording_policy": "started",
            "guest_policy": "invite_only",
            "e2ee_required": true,
            "max_participants": 500,
            "policy_epoch": 8,
            "updated_by": "host",
            "signature": "sig-policy",
            "updated_at_ms": 42
          }
        }
        """;

        var policyFrame = ProtocolCodec.DecodeFrame(policyPayload);
        var policy = Assert.IsType<ProtocolFrame.SessionPolicy>(policyFrame);
        Assert.True(policy.RoomLock);
        Assert.True(policy.WaitingRoomEnabled);
        Assert.Equal(RecordingState.Started, policy.RecordingPolicy);
        Assert.Equal(GuestPolicy.InviteOnly, policy.GuestPolicy);
        Assert.Equal(8, policy.PolicyEpoch);
        Assert.Equal("host", policy.UpdatedBy);

        const string paymentPayload = """
        {
          "kind": "payment_settlement",
          "payment_settlement": {
            "status": "not_required"
          }
        }
        """;

        var paymentFrame = ProtocolCodec.DecodeFrame(paymentPayload);
        var payment = Assert.IsType<ProtocolFrame.PaymentSettlement>(paymentFrame);
        Assert.Equal(PaymentSettlementStatus.NotRequired, payment.Status);
    }

    [Fact]
    public void CodecDecodesDeviceCapabilityAndPongFromNestedPayload()
    {
        const string capabilityPayload = """
        {
          "kind": "device_capability",
          "device_capability": {
            "participant_id": "windows-1",
            "codecs": ["h264", "vp9"],
            "hdr_capture": true,
            "hdr_render": false,
            "max_streams": 3,
            "updated_at_ms": 11
          }
        }
        """;

        var capabilityFrame = ProtocolCodec.DecodeFrame(capabilityPayload);
        var capability = Assert.IsType<ProtocolFrame.DeviceCapability>(capabilityFrame);
        Assert.Equal("windows-1", capability.ParticipantId);
        Assert.Equal(new[] { "h264", "vp9" }, capability.Codecs);
        Assert.True(capability.HdrCapture);
        Assert.False(capability.HdrRender);
        Assert.Equal(3, capability.MaxStreams);
        Assert.Equal(11, capability.UpdatedAtMs);

        const string pongPayload = """
        {
          "kind": "pong",
          "pong": {
            "sent_at_ms": 12
          }
        }
        """;

        var pongFrame = ProtocolCodec.DecodeFrame(pongPayload);
        var pong = Assert.IsType<ProtocolFrame.Pong>(pongFrame);
        Assert.Equal(12, pong.SentAtMs);
    }

    [Fact]
    public void SessionPolicySignedBySystemIsAccepted()
    {
        var state = ProtocolSessionState.Initial(new MeetingConfig
        {
            PreferWebFallbackOnPolicyFailure = false
        });

        var connected = ProtocolReducer.Reduce(
            state,
            new ProtocolEvent.FrameReceived(new ProtocolFrame.HandshakeAck("s3", "r3", 1)),
            nowMs: 1);

        var next = ProtocolReducer.Reduce(
            connected,
            new ProtocolEvent.FrameReceived(new ProtocolFrame.SessionPolicy(
                RoomLock: true,
                WaitingRoomEnabled: true,
                RecordingPolicy: RecordingState.Started,
                GuestPolicy: GuestPolicy.InviteOnly,
                E2eeRequired: false,
                MaxParticipants: 450,
                PolicyEpoch: 10,
                UpdatedBy: "system",
                Signature: "sig-system",
                UpdatedAtMs: 2)),
            nowMs: 2);

        Assert.Equal(ConnectionPhase.Connected, next.ConnectionPhase);
        Assert.True(next.RoomLocked);
        Assert.True(next.WaitingRoomEnabled);
        Assert.Equal(GuestPolicy.InviteOnly, next.GuestPolicy);
        Assert.Null(next.LastError);
    }

    [Fact]
    public void RuntimeSchedulesReconnectWithDeterministicBackoff()
    {
        var runtime = new SessionRuntime(new MeetingConfig(), new long[] { 1_000, 2_000 });
        runtime.ConnectRequested(nowMs: 100);

        var directive = runtime.OnTransportFailure("network_unavailable", nowMs: 200);
        var reconnect = Assert.IsType<RuntimeDirective.ReconnectScheduled>(directive);
        Assert.Equal(1, reconnect.Attempt);
        Assert.Equal(1_200, reconnect.DueAtMs);
        Assert.Equal(1_200, runtime.ReconnectDueAtMs);
        Assert.False(runtime.TakeReconnectIfDue(1_199));
        Assert.True(runtime.TakeReconnectIfDue(1_200));
        Assert.Equal(ConnectionPhase.Connecting, runtime.State.ConnectionPhase);
    }

    [Fact]
    public void RuntimeBackgroundDefersReconnectUntilForegrounded()
    {
        var runtime = new SessionRuntime(new MeetingConfig(), new long[] { 1_000, 2_000 });
        runtime.ConnectRequested(nowMs: 0);

        var directive = runtime.OnTransportFailure("network_unavailable", nowMs: 200);
        Assert.IsType<RuntimeDirective.ReconnectScheduled>(directive);
        Assert.Equal(1_200, runtime.ReconnectDueAtMs);

        runtime.OnAppBackgrounded(nowMs: 300);
        Assert.Null(runtime.ReconnectDueAtMs);
        Assert.False(runtime.TakeReconnectIfDue(1_200));

        runtime.OnAppForegrounded(nowMs: 1_300);
        Assert.Equal(ConnectionPhase.Connecting, runtime.State.ConnectionPhase);
    }

    [Fact]
    public void RuntimeConnectivityRestoreDefersWhileBackgrounded()
    {
        var runtime = new SessionRuntime(new MeetingConfig(), new long[] { 1_000 });
        runtime.ConnectRequested(nowMs: 0);
        runtime.OnAppBackgrounded(nowMs: 1);

        runtime.OnConnectivityChanged(available: true, nowMs: 2);
        Assert.NotEqual(ConnectionPhase.Connecting, runtime.State.ConnectionPhase);

        runtime.OnAppForegrounded(nowMs: 3);
        Assert.Equal(ConnectionPhase.Connecting, runtime.State.ConnectionPhase);
    }

    [Fact]
    public void RuntimeAudioInterruptionHooksEmitTelemetryAndReconnect()
    {
        var telemetry = new InMemoryMeetingTelemetrySink();
        var runtime = new SessionRuntime(new MeetingConfig(), new long[] { 1_000 }, telemetry);
        runtime.ConnectRequested(nowMs: 0);

        runtime.OnAudioInterruptionBegan(nowMs: 10);
        runtime.OnAudioInterruptionEnded(shouldReconnect: true, nowMs: 20);

        Assert.Equal(ConnectionPhase.Connecting, runtime.State.ConnectionPhase);
        Assert.Contains(
            telemetry.Events,
            @event => @event.Category == MeetingTelemetryCategory.ConnectionLifecycle &&
                      @event.Name == "audio_interruption_began");
        Assert.Contains(
            telemetry.Events,
            @event => @event.Category == MeetingTelemetryCategory.ConnectionLifecycle &&
                      @event.Name == "audio_interruption_ended" &&
                      @event.Attributes.TryGetValue("should_reconnect", out var value) &&
                      value == "true");
    }

    [Fact]
    public void RuntimeAudioRouteChangeEmitsTelemetry()
    {
        var telemetry = new InMemoryMeetingTelemetrySink();
        var runtime = new SessionRuntime(new MeetingConfig(), new long[] { 1_000 }, telemetry);

        runtime.OnAudioRouteChanged("becoming_noisy", nowMs: 10);

        Assert.Contains(
            telemetry.Events,
            @event => @event.Category == MeetingTelemetryCategory.ConnectionLifecycle &&
                      @event.Name == "audio_route_changed" &&
                      @event.Attributes.TryGetValue("reason", out var value) &&
                      value == "becoming_noisy");
    }

    [Fact]
    public void RuntimeActivatesFallbackAfterBackoffExhaustion()
    {
        var runtime = new SessionRuntime(new MeetingConfig(), new long[] { 50 });
        runtime.ConnectRequested(nowMs: 0);

        var firstDirective = runtime.OnTransportFailure("socket_closed", nowMs: 10);
        var firstReconnect = Assert.IsType<RuntimeDirective.ReconnectScheduled>(firstDirective);
        Assert.Equal(1, firstReconnect.Attempt);
        Assert.Equal(60, firstReconnect.DueAtMs);
        Assert.True(runtime.TakeReconnectIfDue(60));

        var secondDirective = runtime.OnTransportFailure("socket_closed", nowMs: 70);
        var fallback = Assert.IsType<RuntimeDirective.FallbackActivated>(secondDirective);
        Assert.Contains("Reconnect exhausted", fallback.Reason);
        Assert.True(runtime.State.Fallback.Active);
        Assert.Equal(ConnectionPhase.FallbackActive, runtime.State.ConnectionPhase);
    }

    [Fact]
    public void RuntimeBuildsHandshakeCapabilityAndPaymentFrames()
    {
        var runtime = new SessionRuntime(new MeetingConfig
        {
            RoomId = "ga-room",
            ParticipantId = "windows-guest-1",
            ParticipantName = "Windows Guest",
            RequirePaymentSettlement = true
        });
        runtime.ConnectRequested(nowMs: 0);
        runtime.OnFrame(new ProtocolFrame.HandshakeAck("s1", "resume-1", 1), nowMs: 1);

        var frames = runtime.OnTransportConnected(nowMs: 2).ToArray();
        Assert.Equal(3, frames.Length);

        var handshake = Assert.IsType<ProtocolFrame.Handshake>(frames[0]);
        Assert.Equal("ga-room", handshake.RoomId);
        Assert.Equal("windows-guest-1", handshake.ParticipantId);
        Assert.Equal("Windows Guest", handshake.ParticipantName);
        Assert.Equal("resume-1", handshake.ResumeToken);
        Assert.Equal(MediaProfile.Hdr, handshake.PreferredProfile);
        Assert.True(handshake.HdrCapture);
        Assert.True(handshake.HdrRender);
        var capability = Assert.IsType<ProtocolFrame.DeviceCapability>(frames[1]);
        Assert.Equal("windows-guest-1", capability.ParticipantId);
        Assert.True(capability.HdrCapture);
        Assert.True(capability.HdrRender);
        var paymentPolicy = Assert.IsType<ProtocolFrame.PaymentPolicy>(frames[2]);
        Assert.True(paymentPolicy.Required);
        Assert.Equal("nexus://payment-policy", paymentPolicy.DestinationAccount);
    }

    [Fact]
    public void RuntimeBuildsSdrHandshakeWhenHdrCapabilitiesDisabled()
    {
        var runtime = new SessionRuntime(new MeetingConfig
        {
            ParticipantId = "windows-sdr-guest",
            SupportsHdrCapture = false,
            SupportsHdrRender = false
        });
        runtime.ConnectRequested(nowMs: 0);
        runtime.OnFrame(new ProtocolFrame.HandshakeAck("s2", "resume-2", 1), nowMs: 1);

        var frames = runtime.OnTransportConnected(nowMs: 2).ToArray();
        Assert.Equal(2, frames.Length);

        var handshake = Assert.IsType<ProtocolFrame.Handshake>(frames[0]);
        Assert.Equal("windows-sdr-guest", handshake.ParticipantId);
        Assert.Equal(MediaProfile.Sdr, handshake.PreferredProfile);
        Assert.False(handshake.HdrCapture);
        Assert.False(handshake.HdrRender);

        var capability = Assert.IsType<ProtocolFrame.DeviceCapability>(frames[1]);
        Assert.Equal("windows-sdr-guest", capability.ParticipantId);
        Assert.False(capability.HdrCapture);
        Assert.False(capability.HdrRender);
    }

    [Fact]
    public void RuntimeResolvesParticipantIdFromParticipantNameWhenMissing()
    {
        var runtime = new SessionRuntime(new MeetingConfig
        {
            ParticipantId = "   ",
            ParticipantName = "Windows QA 42"
        });
        runtime.ConnectRequested(nowMs: 0);
        runtime.OnFrame(new ProtocolFrame.HandshakeAck("s3", "resume-3", 1), nowMs: 1);

        var frames = runtime.OnTransportConnected(nowMs: 2).ToArray();
        Assert.Equal(2, frames.Length);

        var handshake = Assert.IsType<ProtocolFrame.Handshake>(frames[0]);
        Assert.Equal("windows-qa-42", handshake.ParticipantId);

        var capability = Assert.IsType<ProtocolFrame.DeviceCapability>(frames[1]);
        Assert.Equal("windows-qa-42", capability.ParticipantId);
    }

    [Fact]
    public void RuntimeNormalizesExplicitParticipantIdToAsciiSubset()
    {
        var runtime = new SessionRuntime(new MeetingConfig
        {
            ParticipantId = " MÜNCHEN_42 ",
            ParticipantName = "Windows Guest"
        });
        runtime.ConnectRequested(nowMs: 0);
        runtime.OnFrame(new ProtocolFrame.HandshakeAck("s4", "resume-4", 1), nowMs: 1);

        var frames = runtime.OnTransportConnected(nowMs: 2).ToArray();
        Assert.Equal(2, frames.Length);

        var handshake = Assert.IsType<ProtocolFrame.Handshake>(frames[0]);
        Assert.Equal("mnchen_42", handshake.ParticipantId);

        var capability = Assert.IsType<ProtocolFrame.DeviceCapability>(frames[1]);
        Assert.Equal("mnchen_42", capability.ParticipantId);
    }

    [Fact]
    public void RuntimeFallsBackToParticipantWhenNormalizedNameIsEmpty()
    {
        var runtime = new SessionRuntime(new MeetingConfig
        {
            ParticipantId = "",
            ParticipantName = "東京"
        });
        runtime.ConnectRequested(nowMs: 0);
        runtime.OnFrame(new ProtocolFrame.HandshakeAck("s5", "resume-5", 1), nowMs: 1);

        var frames = runtime.OnTransportConnected(nowMs: 2).ToArray();
        Assert.Equal(2, frames.Length);

        var handshake = Assert.IsType<ProtocolFrame.Handshake>(frames[0]);
        Assert.Equal("participant", handshake.ParticipantId);

        var capability = Assert.IsType<ProtocolFrame.DeviceCapability>(frames[1]);
        Assert.Equal("participant", capability.ParticipantId);
    }

    [Fact]
    public void RuntimeRepliesToPingWithPong()
    {
        var runtime = new SessionRuntime(new MeetingConfig());
        var outbound = runtime.OnFrame(new ProtocolFrame.Ping(10), nowMs: 20);
        var pong = Assert.Single(outbound);
        var frame = Assert.IsType<ProtocolFrame.Pong>(pong);
        Assert.Equal(20, frame.SentAtMs);
    }

    [Fact]
    public void RuntimeAcknowledgesE2eeKeyEpoch()
    {
        var runtime = new SessionRuntime(new MeetingConfig
        {
            ParticipantId = "windows-guest-1",
            RequireSignedModeration = true
        });

        var outbound = runtime.OnFrame(
            new ProtocolFrame.E2eeKeyEpoch(
                Epoch: 3,
                IssuedBy: "host",
                Signature: "sig-3",
                SentAtMs: 10),
            nowMs: 20).ToArray();

        var ack = Assert.Single(outbound);
        var frame = Assert.IsType<ProtocolFrame.KeyRotationAck>(ack);
        Assert.Equal("windows-guest-1", frame.ParticipantId);
        Assert.Equal(3, frame.AckEpoch);
        Assert.Equal(20, frame.ReceivedAtMs);
        Assert.Equal(3, runtime.State.E2eeState.LastAckEpoch);
    }

    [Fact]
    public void RuntimeDoesNotAcknowledgeUnsignedE2eeKeyEpochWhenSignaturesRequired()
    {
        var runtime = new SessionRuntime(new MeetingConfig
        {
            ParticipantId = "windows-guest-1",
            RequireSignedModeration = true
        });

        _ = runtime.OnFrame(
            new ProtocolFrame.E2eeKeyEpoch(
                Epoch: 3,
                IssuedBy: "host",
                Signature: "sig-3",
                SentAtMs: 10),
            nowMs: 20);
        Assert.Equal(3, runtime.State.E2eeState.LastAckEpoch);

        var outbound = runtime.OnFrame(
            new ProtocolFrame.E2eeKeyEpoch(
                Epoch: 2,
                IssuedBy: "host",
                Signature: null,
                SentAtMs: 30),
            nowMs: 40).ToArray();

        Assert.Empty(outbound);
        Assert.Equal(3, runtime.State.E2eeState.LastAckEpoch);
        Assert.Equal("e2ee_signature_missing", runtime.State.LastError?.Code);
    }

    [Fact]
    public void RuntimeAcknowledgesE2eeKeyEpochWithResolvedParticipantIdWhenMissing()
    {
        var runtime = new SessionRuntime(new MeetingConfig
        {
            ParticipantId = "",
            ParticipantName = "Windows Ops Guest"
        });

        var outbound = runtime.OnFrame(
            new ProtocolFrame.E2eeKeyEpoch(
                Epoch: 4,
                IssuedBy: "host",
                Signature: "sig-4",
                SentAtMs: 10),
            nowMs: 20).ToArray();

        var ack = Assert.Single(outbound);
        var frame = Assert.IsType<ProtocolFrame.KeyRotationAck>(ack);
        Assert.Equal("windows-ops-guest", frame.ParticipantId);
        Assert.Equal(4, frame.AckEpoch);
    }

    [Fact]
    public void RuntimeManualDisconnectSuppressesReconnectSchedule()
    {
        var runtime = new SessionRuntime(new MeetingConfig());
        runtime.ConnectRequested(nowMs: 0);
        runtime.OnManualDisconnect(nowMs: 1);

        var directive = runtime.OnTransportFailure("network", nowMs: 2);
        Assert.IsType<RuntimeDirective.None>(directive);
        Assert.Null(runtime.ReconnectDueAtMs);
    }

    [Fact]
    public void RecoverFromFallbackResetsReconnectBackoffAttempt()
    {
        var runtime = new SessionRuntime(new MeetingConfig(), new long[] { 50 });
        runtime.ConnectRequested(nowMs: 0);

        var firstDirective = runtime.OnTransportFailure("socket_closed", nowMs: 10);
        var firstReconnect = Assert.IsType<RuntimeDirective.ReconnectScheduled>(firstDirective);
        Assert.Equal(1, firstReconnect.Attempt);
        Assert.Equal(60, firstReconnect.DueAtMs);
        Assert.True(runtime.TakeReconnectIfDue(60));

        var exhaustedDirective = runtime.OnTransportFailure("socket_closed", nowMs: 70);
        Assert.IsType<RuntimeDirective.FallbackActivated>(exhaustedDirective);
        Assert.True(runtime.State.Fallback.Active);

        runtime.RecoverFromFallback(nowMs: 100);
        Assert.False(runtime.State.Fallback.Active);
        Assert.Equal(ConnectionPhase.Connecting, runtime.State.ConnectionPhase);

        var afterRecoveryDirective = runtime.OnTransportFailure("socket_closed", nowMs: 110);
        var recoveryReconnect = Assert.IsType<RuntimeDirective.ReconnectScheduled>(afterRecoveryDirective);
        Assert.Equal(1, recoveryReconnect.Attempt);
        Assert.Equal(160, recoveryReconnect.DueAtMs);
    }

    [Fact]
    public void RuntimeEmitsPolicyFailureAndFallbackTelemetryEvents()
    {
        var telemetry = new InMemoryMeetingTelemetrySink();
        var runtime = new SessionRuntime(
            new MeetingConfig
            {
                RequirePaymentSettlement = true,
                PreferWebFallbackOnPolicyFailure = true
            },
            new long[] { 1_000 },
            telemetry);

        runtime.ConnectRequested(nowMs: 0);
        _ = runtime.OnFrame(
            new ProtocolFrame.PaymentPolicy(
                Required: true,
                DestinationAccount: "nexus://dest"),
            nowMs: 10);

        var policyEvent = telemetry.Events.FirstOrDefault(
            @event => @event.Category == MeetingTelemetryCategory.PolicyFailure &&
                      @event.Name == "payment_settlement_required");
        Assert.NotNull(policyEvent);
        Assert.Equal("payment_settlement_required", policyEvent!.Attributes["code"]);

        var fallbackEvent = telemetry.Events.FirstOrDefault(
            @event => @event.Category == MeetingTelemetryCategory.FallbackLifecycle &&
                      @event.Name == "fallback_activated");
        Assert.NotNull(fallbackEvent);
        Assert.Equal("policy:payment_settlement_required", fallbackEvent!.Attributes["reason"]);
    }

    [Fact]
    public void RuntimeEmitsFallbackRecoveredRtoTelemetryEvent()
    {
        var telemetry = new InMemoryMeetingTelemetrySink();
        var runtime = new SessionRuntime(
            new MeetingConfig(),
            Array.Empty<long>(),
            telemetry);

        runtime.ConnectRequested(nowMs: 0);
        var directive = runtime.OnTransportFailure("socket_closed", nowMs: 10);
        Assert.IsType<RuntimeDirective.FallbackActivated>(directive);
        runtime.RecoverFromFallback(nowMs: 120);

        var recoveredEvent = telemetry.Events.FirstOrDefault(
            @event => @event.Category == MeetingTelemetryCategory.FallbackLifecycle &&
                      @event.Name == "fallback_recovered");
        Assert.NotNull(recoveredEvent);
        Assert.Equal("110", recoveredEvent!.Attributes["rto_ms"]);
    }
}
