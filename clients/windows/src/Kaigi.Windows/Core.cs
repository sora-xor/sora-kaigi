using System.Text;
using System.Text.Json;
using System.Text.Json.Serialization;

namespace Kaigi.Windows;

public enum ConnectionPhase
{
    Disconnected,
    Connecting,
    Connected,
    Degraded,
    FallbackActive,
    Error
}

public enum SessionErrorCategory
{
    ProtocolFailure,
    PolicyFailure,
    TransportFailure
}

public sealed record SessionError(SessionErrorCategory Category, string Code, string Message, long AtMs);

public enum ParticipantRole
{
    Host,
    CoHost,
    Participant,
    Guest
}

public sealed record Participant(
    string Id,
    string DisplayName,
    ParticipantRole Role,
    bool Muted,
    bool VideoEnabled,
    bool ShareEnabled,
    bool WaitingRoom);

public sealed record RoleChange(string ParticipantId, ParticipantRole Role);

public enum ModerationAction
{
    Mute,
    VideoOff,
    StopShare,
    Kick,
    AdmitFromWaiting,
    DenyFromWaiting
}

public enum MediaProfile
{
    Sdr,
    Hdr
}

public sealed record MediaProfileState(
    MediaProfile PreferredProfile,
    MediaProfile NegotiatedProfile,
    string ColorPrimaries,
    string TransferFunction,
    string Codec)
{
    public static MediaProfileState Default { get; } = new(
        PreferredProfile: MediaProfile.Sdr,
        NegotiatedProfile: MediaProfile.Sdr,
        ColorPrimaries: "bt709",
        TransferFunction: "gamma",
        Codec: "h264");
}

public enum RecordingState
{
    Stopped,
    Started
}

public enum GuestPolicy
{
    Open,
    InviteOnly,
    Blocked
}

public sealed record PermissionSnapshot(IReadOnlyList<string> EffectivePermissions, int Epoch);

public enum PaymentSettlementStatus
{
    NotRequired,
    Pending,
    Settled,
    Blocked
}

public sealed record PaymentState(bool Required, string? Destination, PaymentSettlementStatus SettlementStatus)
{
    public static PaymentState Default { get; } = new(Required: false, Destination: null, SettlementStatus: PaymentSettlementStatus.NotRequired);
}

public sealed record E2eeState(int CurrentEpoch, int LastAckEpoch)
{
    public static E2eeState Default { get; } = new(CurrentEpoch: 0, LastAckEpoch: 0);
}

public sealed record FallbackState(bool Active, string? Reason, long? ActivatedAtMs, long? RecoveredAtMs, long? LastRtoMs)
{
    public static FallbackState Default { get; } = new(false, null, null, null, null);
}

public sealed class MeetingConfig
{
    public string SignalingUrl { get; init; } = "ws://127.0.0.1:9000";
    public string FallbackUrl { get; init; } = "https://example.com/fallback";
    public string RoomId { get; init; } = "ga-room";
    public string ParticipantId { get; init; } = "windows-guest-1";
    public string ParticipantName { get; init; } = "Windows Guest";
    public string? WalletIdentity { get; init; }
    public bool RequireSignedModeration { get; init; } = true;
    public bool RequirePaymentSettlement { get; init; }
    public bool PreferWebFallbackOnPolicyFailure { get; init; } = true;
    public bool SupportsHdrCapture { get; init; } = true;
    public bool SupportsHdrRender { get; init; } = true;
}

public sealed record ProtocolSessionState(
    MeetingConfig Config,
    ConnectionPhase ConnectionPhase,
    bool HandshakeComplete,
    string? ResumeToken,
    IReadOnlyDictionary<string, Participant> Participants,
    IReadOnlyDictionary<string, PermissionSnapshot> PermissionSnapshots,
    long PresenceSequence,
    bool RoomLocked,
    bool WaitingRoomEnabled,
    GuestPolicy GuestPolicy,
    bool E2eeRequired,
    int MaxParticipants,
    int PolicyEpoch,
    MediaProfileState MediaProfile,
    RecordingState RecordingNotice,
    E2eeState E2eeState,
    PaymentState PaymentState,
    FallbackState Fallback,
    SessionError? LastError)
{
    public static ProtocolSessionState Initial(MeetingConfig config) => new(
        Config: config,
        ConnectionPhase: ConnectionPhase.Disconnected,
        HandshakeComplete: false,
        ResumeToken: null,
        Participants: new Dictionary<string, Participant>(),
        PermissionSnapshots: new Dictionary<string, PermissionSnapshot>(),
        PresenceSequence: 0,
        RoomLocked: false,
        WaitingRoomEnabled: false,
        GuestPolicy: GuestPolicy.Open,
        E2eeRequired: true,
        MaxParticipants: 300,
        PolicyEpoch: 0,
        MediaProfile: MediaProfileState.Default,
        RecordingNotice: RecordingState.Stopped,
        E2eeState: E2eeState.Default,
        PaymentState: PaymentState.Default,
        Fallback: FallbackState.Default,
        LastError: null);
}

public abstract record ProtocolEvent
{
    public sealed record ConnectRequested : ProtocolEvent;
    public sealed record TransportConnected : ProtocolEvent;
    public sealed record TransportDisconnected(string Reason) : ProtocolEvent;
    public sealed record TransportFailure(string Message) : ProtocolEvent;
    public sealed record FrameReceived(ProtocolFrame Frame) : ProtocolEvent;
    public sealed record FrameSendFailed(string Message) : ProtocolEvent;
    public sealed record ManualDisconnected : ProtocolEvent;
    public sealed record FallbackActivated(string Reason) : ProtocolEvent;
    public sealed record FallbackRecovered : ProtocolEvent;
    public sealed record ConfigUpdated(MeetingConfig Config) : ProtocolEvent;
}

public abstract record ProtocolFrame
{
    public sealed record Handshake(
        string RoomId,
        string ParticipantId,
        string ParticipantName,
        string? WalletIdentity,
        string? ResumeToken,
        MediaProfile PreferredProfile,
        bool HdrCapture,
        bool HdrRender,
        long SentAtMs) : ProtocolFrame;
    public sealed record HandshakeAck(string SessionId, string ResumeToken, long AcceptedAtMs) : ProtocolFrame;
    public sealed record PresenceDelta(IReadOnlyList<Participant> Joined, IReadOnlyList<string> Left, IReadOnlyList<RoleChange> RoleChanges, long Sequence) : ProtocolFrame;
    public sealed record RoleGrant(string TargetParticipantId, ParticipantRole Role, string GrantedBy, string? Signature, long IssuedAtMs) : ProtocolFrame;
    public sealed record RoleRevoke(string TargetParticipantId, ParticipantRole Role, string RevokedBy, string? Signature, long IssuedAtMs) : ProtocolFrame;
    public sealed record PermissionsSnapshot(string ParticipantId, IReadOnlyList<string> EffectivePermissions, int Epoch) : ProtocolFrame;
    public sealed record ModerationSigned(string TargetParticipantId, ModerationAction Action, string IssuedBy, string? Signature, long SentAtMs) : ProtocolFrame;
    public sealed record SessionPolicy(bool RoomLock, bool WaitingRoomEnabled, RecordingState RecordingPolicy, GuestPolicy GuestPolicy, bool E2eeRequired, int MaxParticipants, int PolicyEpoch, string UpdatedBy, string? Signature, long UpdatedAtMs) : ProtocolFrame;
    public sealed record DeviceCapability(string ParticipantId, IReadOnlyList<string> Codecs, bool HdrCapture, bool HdrRender, int MaxStreams, long UpdatedAtMs) : ProtocolFrame;
    public sealed record MediaProfileNegotiation(MediaProfile PreferredProfile, MediaProfile NegotiatedProfile, string ColorPrimaries, string TransferFunction, string Codec) : ProtocolFrame;
    public sealed record RecordingNotice(string ParticipantId, RecordingState State, string Mode, string PolicyBasis, long IssuedAtMs, string IssuedBy) : ProtocolFrame;
    public sealed record E2eeKeyEpoch(int Epoch, string IssuedBy, string? Signature, long SentAtMs) : ProtocolFrame;
    public sealed record KeyRotationAck(string ParticipantId, int AckEpoch, long ReceivedAtMs) : ProtocolFrame;
    public sealed record PaymentPolicy(bool Required, string? DestinationAccount) : ProtocolFrame;
    public sealed record PaymentSettlement(PaymentSettlementStatus Status) : ProtocolFrame;
    public sealed record Ping(long SentAtMs) : ProtocolFrame;
    public sealed record Pong(long SentAtMs) : ProtocolFrame;
    public sealed record Error(SessionErrorCategory Category, string Code, string Message) : ProtocolFrame;
}

public static class ProtocolReducer
{
    public static ProtocolSessionState Reduce(ProtocolSessionState state, ProtocolEvent @event, long nowMs)
    {
        return @event switch
        {
            ProtocolEvent.ConnectRequested => state with { ConnectionPhase = ConnectionPhase.Connecting, HandshakeComplete = false, LastError = null },
            ProtocolEvent.TransportConnected => state with { ConnectionPhase = ConnectionPhase.Connecting, LastError = null },
            ProtocolEvent.TransportDisconnected disconnected =>
                state.Fallback.Active
                    ? state with { ConnectionPhase = ConnectionPhase.FallbackActive, HandshakeComplete = false }
                    : state with
                    {
                        ConnectionPhase = ConnectionPhase.Degraded,
                        HandshakeComplete = false,
                        LastError = new SessionError(SessionErrorCategory.TransportFailure, "transport_disconnected", disconnected.Reason, nowMs)
                    },
            ProtocolEvent.TransportFailure failure =>
                state.Fallback.Active
                    ? state with { ConnectionPhase = ConnectionPhase.FallbackActive, HandshakeComplete = false }
                    : state with
                    {
                        ConnectionPhase = ConnectionPhase.Degraded,
                        HandshakeComplete = false,
                        LastError = new SessionError(SessionErrorCategory.TransportFailure, "transport_failure", failure.Message, nowMs)
                    },
            ProtocolEvent.FrameSendFailed sendFailed =>
                state.Fallback.Active
                    ? state with { ConnectionPhase = ConnectionPhase.FallbackActive }
                    : state with
                    {
                        ConnectionPhase = ConnectionPhase.Degraded,
                        LastError = new SessionError(SessionErrorCategory.TransportFailure, "send_failed", sendFailed.Message, nowMs)
                    },
            ProtocolEvent.ManualDisconnected => state with { ConnectionPhase = ConnectionPhase.Disconnected, HandshakeComplete = false, LastError = null },
            ProtocolEvent.FallbackActivated fallback => state with
            {
                ConnectionPhase = ConnectionPhase.FallbackActive,
                Fallback = state.Fallback with { Active = true, Reason = fallback.Reason, ActivatedAtMs = nowMs },
                LastError = new SessionError(SessionErrorCategory.TransportFailure, "fallback_activated", fallback.Reason, nowMs)
            },
            ProtocolEvent.FallbackRecovered => state with
            {
                ConnectionPhase = ConnectionPhase.Disconnected,
                Fallback = state.Fallback with
                {
                    Active = false,
                    Reason = null,
                    RecoveredAtMs = nowMs,
                    LastRtoMs = state.Fallback.ActivatedAtMs.HasValue ? Math.Max(0, nowMs - state.Fallback.ActivatedAtMs.Value) : null
                }
            },
            ProtocolEvent.ConfigUpdated update => EnforcePaymentPolicy(state with { Config = update.Config }, nowMs),
            ProtocolEvent.FrameReceived received => ReduceFrame(state, received.Frame, nowMs),
            _ => state
        };
    }

    private static ProtocolSessionState ReduceFrame(ProtocolSessionState state, ProtocolFrame frame, long nowMs)
    {
        return frame switch
        {
            ProtocolFrame.HandshakeAck ack => state with
            {
                HandshakeComplete = true,
                ResumeToken = ack.ResumeToken,
                ConnectionPhase = state.Fallback.Active ? ConnectionPhase.FallbackActive : ConnectionPhase.Connected,
                LastError = null
            },
            ProtocolFrame.PresenceDelta delta => ApplyPresenceDelta(state, delta),
            ProtocolFrame.RoleGrant grant => ApplyRoleGrant(state, grant, nowMs),
            ProtocolFrame.RoleRevoke revoke => ApplyRoleRevoke(state, revoke, nowMs),
            ProtocolFrame.PermissionsSnapshot snapshot => ApplyPermissionSnapshot(state, snapshot),
            ProtocolFrame.ModerationSigned moderation => ApplyModeration(state, moderation, nowMs),
            ProtocolFrame.SessionPolicy policy => ApplySessionPolicy(state, policy, nowMs),
            ProtocolFrame.MediaProfileNegotiation media => state with
            {
                MediaProfile = new MediaProfileState(media.PreferredProfile, media.NegotiatedProfile, media.ColorPrimaries, media.TransferFunction, media.Codec),
                ConnectionPhase = media.PreferredProfile == MediaProfile.Hdr && media.NegotiatedProfile == MediaProfile.Sdr
                    ? ConnectionPhase.Degraded
                    : state.HandshakeComplete && !state.Fallback.Active
                        ? ConnectionPhase.Connected
                        : state.ConnectionPhase
            },
            ProtocolFrame.RecordingNotice recording => state with { RecordingNotice = recording.State },
            ProtocolFrame.E2eeKeyEpoch epoch => ApplyE2eeEpoch(state, epoch, nowMs),
            ProtocolFrame.KeyRotationAck ack => state with
            {
                E2eeState = state.E2eeState with
                {
                    LastAckEpoch = Math.Max(state.E2eeState.LastAckEpoch, ack.AckEpoch)
                }
            },
            ProtocolFrame.PaymentPolicy paymentPolicy => EnforcePaymentPolicy(state with
            {
                PaymentState = new PaymentState(paymentPolicy.Required, paymentPolicy.DestinationAccount, paymentPolicy.Required ? PaymentSettlementStatus.Pending : PaymentSettlementStatus.NotRequired)
            }, nowMs),
            ProtocolFrame.PaymentSettlement payment => EnforcePaymentPolicy(state with
            {
                PaymentState = state.PaymentState with { SettlementStatus = payment.Status }
            }, nowMs),
            ProtocolFrame.Error error => state with
            {
                ConnectionPhase = state.Fallback.Active
                    ? ConnectionPhase.FallbackActive
                    : error.Category == SessionErrorCategory.PolicyFailure
                        ? ConnectionPhase.Error
                        : ConnectionPhase.Degraded,
                LastError = new SessionError(error.Category, error.Code, error.Message, nowMs)
            },
            ProtocolFrame.Handshake _ => state,
            ProtocolFrame.DeviceCapability _ => state,
            ProtocolFrame.Ping _ => state,
            ProtocolFrame.Pong _ => state,
            _ => state
        };
    }

    private static bool HasRequiredSignature(string? signature, ProtocolSessionState state)
    {
        if (!state.Config.RequireSignedModeration)
        {
            return true;
        }
        return !string.IsNullOrWhiteSpace(signature);
    }

    private static bool ActorAuthorized(string actorId, ProtocolSessionState state)
    {
        if (string.Equals(actorId, "system", StringComparison.Ordinal))
        {
            return true;
        }

        if (!state.Participants.TryGetValue(actorId, out var participant))
        {
            return false;
        }
        return participant.Role is ParticipantRole.Host or ParticipantRole.CoHost;
    }

    private static ProtocolSessionState PolicyReject(ProtocolSessionState state, long nowMs, string code, string message)
    {
        var fallbackRequested = state.Config.PreferWebFallbackOnPolicyFailure;
        var fallbackActive = fallbackRequested || state.Fallback.Active;

        return state with
        {
            ConnectionPhase = fallbackActive ? ConnectionPhase.FallbackActive : ConnectionPhase.Error,
            Fallback = fallbackActive
                ? state.Fallback with
                {
                    Active = true,
                    Reason = state.Fallback.Reason ?? $"policy:{code}",
                    ActivatedAtMs = state.Fallback.ActivatedAtMs ?? nowMs
                }
                : state.Fallback,
            LastError = new SessionError(SessionErrorCategory.PolicyFailure, code, message, nowMs)
        };
    }

    private static ProtocolSessionState EnforcePaymentPolicy(ProtocolSessionState state, long nowMs)
    {
        if (!state.Config.RequirePaymentSettlement || !state.PaymentState.Required)
        {
            return ClearPaymentError(state);
        }

        if (state.PaymentState.SettlementStatus is PaymentSettlementStatus.Settled or PaymentSettlementStatus.NotRequired)
        {
            return ClearPaymentError(state);
        }

        return state.PaymentState.SettlementStatus == PaymentSettlementStatus.Blocked
            ? PolicyReject(state, nowMs, "payment_settlement_blocked", "Payment settlement blocked by policy")
            : PolicyReject(
                state,
                nowMs,
                "payment_settlement_required",
                "Payment settlement required before media/session actions can continue");
    }

    private static ProtocolSessionState ClearPaymentError(ProtocolSessionState state)
    {
        if (state.LastError is null)
        {
            return state;
        }
        if (state.LastError.Category != SessionErrorCategory.PolicyFailure)
        {
            return state;
        }
        if (
            !state.LastError.Code.StartsWith("payment_settlement_", StringComparison.Ordinal) &&
            !string.Equals(state.LastError.Code, "payment_unsettled", StringComparison.Ordinal))
        {
            return state;
        }

        return state with
        {
            LastError = null,
            ConnectionPhase = state.Fallback.Active
                ? ConnectionPhase.FallbackActive
                : state.HandshakeComplete
                    ? ConnectionPhase.Connected
                    : ConnectionPhase.Connecting
        };
    }

    private static ProtocolSessionState ApplyPresenceDelta(ProtocolSessionState state, ProtocolFrame.PresenceDelta delta)
    {
        if (delta.Sequence <= state.PresenceSequence)
        {
            return state;
        }

        var next = state.Participants.ToDictionary(pair => pair.Key, pair => pair.Value);
        foreach (var participant in delta.Joined)
        {
            next[participant.Id] = participant;
        }
        foreach (var left in delta.Left)
        {
            next.Remove(left);
        }
        foreach (var roleChange in delta.RoleChanges)
        {
            if (next.TryGetValue(roleChange.ParticipantId, out var participant))
            {
                next[roleChange.ParticipantId] = participant with { Role = roleChange.Role };
            }
        }

        return state with
        {
            Participants = next,
            PresenceSequence = delta.Sequence
        };
    }

    private static ProtocolSessionState ApplyPermissionSnapshot(ProtocolSessionState state, ProtocolFrame.PermissionsSnapshot snapshot)
    {
        if (state.PermissionSnapshots.TryGetValue(snapshot.ParticipantId, out var prior) && snapshot.Epoch <= prior.Epoch)
        {
            return state;
        }

        var next = state.PermissionSnapshots.ToDictionary(pair => pair.Key, pair => pair.Value);
        next[snapshot.ParticipantId] = new PermissionSnapshot(snapshot.EffectivePermissions, snapshot.Epoch);
        return state with { PermissionSnapshots = next };
    }

    private static ProtocolSessionState ApplyRoleGrant(ProtocolSessionState state, ProtocolFrame.RoleGrant grant, long nowMs)
    {
        if (!HasRequiredSignature(grant.Signature, state))
        {
            return PolicyReject(state, nowMs, "role_grant_signature_missing", "RoleGrant signature is required");
        }
        if (!ActorAuthorized(grant.GrantedBy, state))
        {
            return PolicyReject(state, nowMs, "role_grant_not_authorized", "RoleGrant issuer is not host/co-host");
        }

        var next = state.Participants.ToDictionary(pair => pair.Key, pair => pair.Value);
        if (next.TryGetValue(grant.TargetParticipantId, out var participant))
        {
            next[grant.TargetParticipantId] = participant with { Role = grant.Role };
        }

        return state with { Participants = next };
    }

    private static ProtocolSessionState ApplyRoleRevoke(ProtocolSessionState state, ProtocolFrame.RoleRevoke revoke, long nowMs)
    {
        if (!HasRequiredSignature(revoke.Signature, state))
        {
            return PolicyReject(state, nowMs, "role_revoke_signature_missing", "RoleRevoke signature is required");
        }
        if (!ActorAuthorized(revoke.RevokedBy, state))
        {
            return PolicyReject(state, nowMs, "role_revoke_not_authorized", "RoleRevoke issuer is not host/co-host");
        }

        var next = state.Participants.ToDictionary(pair => pair.Key, pair => pair.Value);
        if (next.TryGetValue(revoke.TargetParticipantId, out var participant) && participant.Role == revoke.Role)
        {
            next[revoke.TargetParticipantId] = participant with { Role = ParticipantRole.Participant };
        }

        return state with { Participants = next };
    }

    private static ProtocolSessionState ApplyModeration(ProtocolSessionState state, ProtocolFrame.ModerationSigned moderation, long nowMs)
    {
        if (!HasRequiredSignature(moderation.Signature, state))
        {
            return PolicyReject(state, nowMs, "moderation_signature_missing", "Moderation signature is required");
        }
        if (!ActorAuthorized(moderation.IssuedBy, state))
        {
            return PolicyReject(state, nowMs, "moderation_not_authorized", "Moderation issuer is not host/co-host");
        }

        var next = state.Participants.ToDictionary(pair => pair.Key, pair => pair.Value);
        if (!next.TryGetValue(moderation.TargetParticipantId, out var participant))
        {
            return state;
        }

        switch (moderation.Action)
        {
            case ModerationAction.Mute:
                next[moderation.TargetParticipantId] = participant with { Muted = true };
                break;
            case ModerationAction.VideoOff:
                next[moderation.TargetParticipantId] = participant with { VideoEnabled = false };
                break;
            case ModerationAction.StopShare:
                next[moderation.TargetParticipantId] = participant with { ShareEnabled = false };
                break;
            case ModerationAction.AdmitFromWaiting:
                next[moderation.TargetParticipantId] = participant with { WaitingRoom = false };
                break;
            case ModerationAction.Kick:
            case ModerationAction.DenyFromWaiting:
                next.Remove(moderation.TargetParticipantId);
                break;
        }

        return state with { Participants = next };
    }

    private static ProtocolSessionState ApplySessionPolicy(ProtocolSessionState state, ProtocolFrame.SessionPolicy policy, long nowMs)
    {
        if (!HasRequiredSignature(policy.Signature, state))
        {
            return PolicyReject(state, nowMs, "session_policy_signature_missing", "SessionPolicy signature is required");
        }
        if (!ActorAuthorized(policy.UpdatedBy, state))
        {
            return PolicyReject(state, nowMs, "session_policy_not_authorized", "SessionPolicy issuer is not host/co-host");
        }
        if (policy.PolicyEpoch < state.PolicyEpoch)
        {
            return state;
        }

        var next = state with
        {
            RoomLocked = policy.RoomLock,
            WaitingRoomEnabled = policy.WaitingRoomEnabled,
            GuestPolicy = policy.GuestPolicy,
            E2eeRequired = policy.E2eeRequired,
            MaxParticipants = policy.MaxParticipants,
            PolicyEpoch = policy.PolicyEpoch,
            RecordingNotice = policy.RecordingPolicy
        };

        return EnforceE2eeEpoch(next, nowMs);
    }

    private static ProtocolSessionState ApplyE2eeEpoch(ProtocolSessionState state, ProtocolFrame.E2eeKeyEpoch epoch, long nowMs)
    {
        if (!HasRequiredSignature(epoch.Signature, state))
        {
            return PolicyReject(state, nowMs, "e2ee_signature_missing", "E2EE key epoch signature is required");
        }
        var next = state with
        {
            E2eeState = state.E2eeState with { CurrentEpoch = Math.Max(state.E2eeState.CurrentEpoch, epoch.Epoch) }
        };
        return EnforceE2eeEpoch(next, nowMs);
    }

    private static ProtocolSessionState EnforceE2eeEpoch(ProtocolSessionState state, long nowMs)
    {
        if (!state.E2eeRequired)
        {
            return ClearE2eeError(state);
        }
        if (state.E2eeState.CurrentEpoch > 0)
        {
            return ClearE2eeError(state);
        }

        return PolicyReject(state, nowMs, "e2ee_epoch_required", "E2EE key epoch is required by session policy");
    }

    private static ProtocolSessionState ClearE2eeError(ProtocolSessionState state)
    {
        if (state.LastError is null)
        {
            return state;
        }
        if (state.LastError.Category != SessionErrorCategory.PolicyFailure || !state.LastError.Code.StartsWith("e2ee_", StringComparison.Ordinal))
        {
            return state;
        }
        return state with
        {
            LastError = null,
            ConnectionPhase = state.Fallback.Active
                ? ConnectionPhase.FallbackActive
                : state.HandshakeComplete
                    ? ConnectionPhase.Connected
                    : ConnectionPhase.Connecting
        };
    }
}

public static class ProtocolCodec
{
    private static readonly JsonSerializerOptions JsonOptions = new()
    {
        PropertyNamingPolicy = JsonNamingPolicy.CamelCase,
        DefaultIgnoreCondition = JsonIgnoreCondition.WhenWritingNull
    };

    public static string EncodeFrame(ProtocolFrame frame)
    {
        var payload = frame switch
        {
            ProtocolFrame.Handshake handshake => new
            {
                kind = "handshake",
                handshake = new
                {
                    roomId = handshake.RoomId,
                    participantId = handshake.ParticipantId,
                    participantName = handshake.ParticipantName,
                    walletIdentity = handshake.WalletIdentity,
                    resumeToken = handshake.ResumeToken,
                    preferredProfile = ToWire(handshake.PreferredProfile),
                    hdrCapture = handshake.HdrCapture,
                    hdrRender = handshake.HdrRender,
                    sentAtMs = handshake.SentAtMs
                }
            },
            ProtocolFrame.HandshakeAck ack => new
            {
                kind = "handshakeAck",
                handshakeAck = new
                {
                    sessionId = ack.SessionId,
                    resumeToken = ack.ResumeToken,
                    acceptedAtMs = ack.AcceptedAtMs
                }
            },
            ProtocolFrame.PresenceDelta delta => new
            {
                kind = "participantPresenceDelta",
                presenceDelta = new
                {
                    joined = delta.Joined.Select(participant => new
                    {
                        id = participant.Id,
                        displayName = participant.DisplayName,
                        role = ToWire(participant.Role),
                        muted = participant.Muted,
                        videoEnabled = participant.VideoEnabled,
                        shareEnabled = participant.ShareEnabled,
                        waitingRoom = participant.WaitingRoom
                    }),
                    left = delta.Left,
                    roleChanges = delta.RoleChanges.Select(change => new
                    {
                        participantId = change.ParticipantId,
                        role = ToWire(change.Role)
                    }),
                    sequence = delta.Sequence
                }
            },
            ProtocolFrame.RoleGrant grant => new
            {
                kind = "roleGrant",
                roleGrant = new
                {
                    targetParticipantId = grant.TargetParticipantId,
                    role = ToWire(grant.Role),
                    grantedBy = grant.GrantedBy,
                    signature = grant.Signature,
                    issuedAtMs = grant.IssuedAtMs
                }
            },
            ProtocolFrame.RoleRevoke revoke => new
            {
                kind = "roleRevoke",
                roleRevoke = new
                {
                    targetParticipantId = revoke.TargetParticipantId,
                    role = ToWire(revoke.Role),
                    revokedBy = revoke.RevokedBy,
                    signature = revoke.Signature,
                    issuedAtMs = revoke.IssuedAtMs
                }
            },
            ProtocolFrame.PermissionsSnapshot snapshot => new
            {
                kind = "permissionsSnapshot",
                permissionsSnapshot = new
                {
                    participantId = snapshot.ParticipantId,
                    effectivePermissions = snapshot.EffectivePermissions,
                    epoch = snapshot.Epoch
                }
            },
            ProtocolFrame.ModerationSigned moderation => new
            {
                kind = "moderationSigned",
                moderationSigned = new
                {
                    targetParticipantId = moderation.TargetParticipantId,
                    action = ToWire(moderation.Action),
                    issuedBy = moderation.IssuedBy,
                    signature = moderation.Signature,
                    sentAtMs = moderation.SentAtMs
                }
            },
            ProtocolFrame.SessionPolicy policy => new
            {
                kind = "sessionPolicy",
                sessionPolicy = new
                {
                    roomLock = policy.RoomLock,
                    waitingRoomEnabled = policy.WaitingRoomEnabled,
                    recordingPolicy = ToWire(policy.RecordingPolicy),
                    guestPolicy = ToWire(policy.GuestPolicy),
                    e2eeRequired = policy.E2eeRequired,
                    maxParticipants = policy.MaxParticipants,
                    policyEpoch = policy.PolicyEpoch,
                    updatedBy = policy.UpdatedBy,
                    signature = policy.Signature,
                    updatedAtMs = policy.UpdatedAtMs
                }
            },
            ProtocolFrame.DeviceCapability capability => new
            {
                kind = "deviceCapability",
                deviceCapability = new
                {
                    participantId = capability.ParticipantId,
                    codecs = capability.Codecs,
                    hdrCapture = capability.HdrCapture,
                    hdrRender = capability.HdrRender,
                    maxStreams = capability.MaxStreams,
                    updatedAtMs = capability.UpdatedAtMs
                }
            },
            ProtocolFrame.MediaProfileNegotiation media => new
            {
                kind = "mediaProfileNegotiation",
                mediaProfileNegotiation = new
                {
                    preferredProfile = ToWire(media.PreferredProfile),
                    negotiatedProfile = ToWire(media.NegotiatedProfile),
                    colorPrimaries = media.ColorPrimaries,
                    transferFunction = media.TransferFunction,
                    codec = media.Codec
                }
            },
            ProtocolFrame.RecordingNotice recording => new
            {
                kind = "recordingNotice",
                recordingNotice = new
                {
                    participantId = recording.ParticipantId,
                    state = ToWire(recording.State),
                    mode = recording.Mode,
                    policyBasis = recording.PolicyBasis,
                    issuedAtMs = recording.IssuedAtMs,
                    issuedBy = recording.IssuedBy
                }
            },
            ProtocolFrame.E2eeKeyEpoch epoch => new
            {
                kind = "e2eeKeyEpoch",
                e2eeKeyEpoch = new
                {
                    epoch = epoch.Epoch,
                    issuedBy = epoch.IssuedBy,
                    signature = epoch.Signature,
                    sentAtMs = epoch.SentAtMs
                }
            },
            ProtocolFrame.KeyRotationAck ack => new
            {
                kind = "keyRotationAck",
                keyRotationAck = new
                {
                    participantId = ack.ParticipantId,
                    ackEpoch = ack.AckEpoch,
                    receivedAtMs = ack.ReceivedAtMs
                }
            },
            ProtocolFrame.PaymentPolicy paymentPolicy => new
            {
                kind = "paymentPolicy",
                paymentPolicy = new
                {
                    required = paymentPolicy.Required,
                    destinationAccount = paymentPolicy.DestinationAccount
                }
            },
            ProtocolFrame.PaymentSettlement payment => new
            {
                kind = "paymentSettlement",
                paymentSettlement = new
                {
                    status = ToWire(payment.Status)
                }
            },
            ProtocolFrame.Ping ping => new
            {
                kind = "ping",
                ping = new
                {
                    sentAtMs = ping.SentAtMs
                }
            },
            ProtocolFrame.Pong pong => new
            {
                kind = "pong",
                pong = new
                {
                    sentAtMs = pong.SentAtMs
                }
            },
            ProtocolFrame.Error error => new
            {
                kind = "error",
                error = new
                {
                    category = ToWire(error.Category),
                    code = error.Code,
                    message = error.Message
                }
            },
            _ => new { kind = "unsupported" }
        };

        return JsonSerializer.Serialize(payload, JsonOptions);
    }

    public static ProtocolFrame DecodeFrame(string payload)
    {
        using var doc = JsonDocument.Parse(payload);
        var root = doc.RootElement;
        var kind = NormalizeKind(GetString(root, "kind") ?? "error");

        return kind switch
        {
            "handshake" => DecodeHandshake(root),
            "handshakeAck" => DecodeHandshakeAck(root),
            "participantPresenceDelta" => DecodePresenceDelta(root),
            "roleGrant" => DecodeRoleGrant(root),
            "roleRevoke" => DecodeRoleRevoke(root),
            "permissionsSnapshot" => DecodePermissionsSnapshot(root),
            "moderationSigned" => DecodeModerationSigned(root),
            "sessionPolicy" => DecodeSessionPolicy(root),
            "deviceCapability" => DecodeDeviceCapability(root),
            "mediaProfileNegotiation" => DecodeMediaProfileNegotiation(root),
            "recordingNotice" => DecodeRecordingNotice(root),
            "e2eeKeyEpoch" => DecodeE2eeKeyEpoch(root),
            "keyRotationAck" => DecodeKeyRotationAck(root),
            "paymentPolicy" => DecodePaymentPolicy(root),
            "paymentSettlement" => DecodePaymentSettlement(root),
            "ping" => DecodePing(root),
            "pong" => DecodePong(root),
            "error" => DecodeError(root),
            _ => new ProtocolFrame.Error(SessionErrorCategory.ProtocolFailure, "unknown_kind", $"Unsupported frame kind: {kind}")
        };
    }

    private static ProtocolFrame DecodeHandshake(JsonElement root)
    {
        var payload = GetPayload(root, "handshake");
        return new ProtocolFrame.Handshake(
            RoomId: GetString(payload, "roomId", "roomID", "room_id") ?? string.Empty,
            ParticipantId: GetString(payload, "participantId", "participantID", "participant_id") ?? string.Empty,
            ParticipantName: GetString(payload, "participantName", "participant_name") ?? string.Empty,
            WalletIdentity: GetString(payload, "walletIdentity", "wallet_identity"),
            ResumeToken: GetString(payload, "resumeToken", "resume_token"),
            PreferredProfile: ParseMediaProfile(GetString(payload, "preferredProfile", "preferred_profile")),
            HdrCapture: GetBool(payload, false, "hdrCapture", "hdr_capture"),
            HdrRender: GetBool(payload, false, "hdrRender", "hdr_render"),
            SentAtMs: GetInt64(payload, 0, "sentAtMs", "sent_at_ms"));
    }

    private static ProtocolFrame DecodeHandshakeAck(JsonElement root)
    {
        var payload = GetPayload(root, "handshakeAck", "handshake_ack");
        return new ProtocolFrame.HandshakeAck(
            SessionId: GetString(payload, "sessionId", "sessionID", "session_id") ?? string.Empty,
            ResumeToken: GetString(payload, "resumeToken", "resume_token") ?? string.Empty,
            AcceptedAtMs: GetInt64(payload, 0, "acceptedAtMs", "accepted_at_ms"));
    }

    private static ProtocolFrame DecodePresenceDelta(JsonElement root)
    {
        var payload = GetPayload(root, "presenceDelta", "participantPresenceDelta", "participant_presence_delta");
        var joined = ParseParticipants(payload, "joined");
        var left = ParseStringArray(payload, "left");
        var roleChanges = ParseRoleChanges(payload, "roleChanges", "role_changes");
        var sequence = GetInt64(payload, 0, "sequence");
        return new ProtocolFrame.PresenceDelta(joined, left, roleChanges, sequence);
    }

    private static ProtocolFrame DecodeRoleGrant(JsonElement root)
    {
        var payload = GetPayload(root, "roleGrant", "role_grant");
        return new ProtocolFrame.RoleGrant(
            TargetParticipantId: GetString(payload, "targetParticipantId", "targetParticipantID", "target_participant_id") ?? string.Empty,
            Role: ParseParticipantRole(GetString(payload, "role")),
            GrantedBy: GetString(payload, "grantedBy", "granted_by") ?? "unknown",
            Signature: GetString(payload, "signature"),
            IssuedAtMs: GetInt64(payload, 0, "issuedAtMs", "issued_at_ms"));
    }

    private static ProtocolFrame DecodeRoleRevoke(JsonElement root)
    {
        var payload = GetPayload(root, "roleRevoke", "role_revoke");
        return new ProtocolFrame.RoleRevoke(
            TargetParticipantId: GetString(payload, "targetParticipantId", "targetParticipantID", "target_participant_id") ?? string.Empty,
            Role: ParseParticipantRole(GetString(payload, "role")),
            RevokedBy: GetString(payload, "revokedBy", "revoked_by") ?? "unknown",
            Signature: GetString(payload, "signature"),
            IssuedAtMs: GetInt64(payload, 0, "issuedAtMs", "issued_at_ms"));
    }

    private static ProtocolFrame DecodePermissionsSnapshot(JsonElement root)
    {
        var payload = GetPayload(root, "permissionsSnapshot", "permissions_snapshot");
        return new ProtocolFrame.PermissionsSnapshot(
            ParticipantId: GetString(payload, "participantId", "participantID", "participant_id") ?? string.Empty,
            EffectivePermissions: ParseStringArray(payload, "effectivePermissions", "effective_permissions"),
            Epoch: GetInt32(payload, 0, "epoch"));
    }

    private static ProtocolFrame DecodeModerationSigned(JsonElement root)
    {
        var payload = GetPayload(root, "moderationSigned", "moderation_signed");
        return new ProtocolFrame.ModerationSigned(
            TargetParticipantId: GetString(payload, "targetParticipantId", "targetParticipantID", "target_participant_id") ?? string.Empty,
            Action: ParseModerationAction(GetString(payload, "action")),
            IssuedBy: GetString(payload, "issuedBy", "issued_by") ?? "unknown",
            Signature: GetString(payload, "signature"),
            SentAtMs: GetInt64(payload, 0, "sentAtMs", "sent_at_ms"));
    }

    private static ProtocolFrame DecodeSessionPolicy(JsonElement root)
    {
        var payload = GetPayload(root, "sessionPolicy", "session_policy");
        return new ProtocolFrame.SessionPolicy(
            RoomLock: GetBool(payload, false, "roomLock", "room_lock"),
            WaitingRoomEnabled: GetBool(payload, false, "waitingRoomEnabled", "waiting_room_enabled"),
            RecordingPolicy: ParseRecordingState(GetString(payload, "recordingPolicy", "recording_policy")),
            GuestPolicy: ParseGuestPolicy(GetString(payload, "guestPolicy", "guest_policy")),
            E2eeRequired: GetBool(payload, true, "e2eeRequired", "e2ee_required"),
            MaxParticipants: GetInt32(payload, 300, "maxParticipants", "max_participants"),
            PolicyEpoch: GetInt32(payload, 0, "policyEpoch", "policy_epoch"),
            UpdatedBy: GetString(payload, "updatedBy", "updated_by") ?? "system",
            Signature: GetString(payload, "signature"),
            UpdatedAtMs: GetInt64(payload, 0, "updatedAtMs", "updated_at_ms"));
    }

    private static ProtocolFrame DecodeDeviceCapability(JsonElement root)
    {
        var payload = GetPayload(root, "deviceCapability", "device_capability");
        return new ProtocolFrame.DeviceCapability(
            ParticipantId: GetString(payload, "participantId", "participantID", "participant_id") ?? string.Empty,
            Codecs: ParseStringArray(payload, "codecs"),
            HdrCapture: GetBool(payload, false, "hdrCapture", "hdr_capture"),
            HdrRender: GetBool(payload, false, "hdrRender", "hdr_render"),
            MaxStreams: GetInt32(payload, 1, "maxStreams", "max_streams"),
            UpdatedAtMs: GetInt64(payload, 0, "updatedAtMs", "updated_at_ms"));
    }

    private static ProtocolFrame DecodeMediaProfileNegotiation(JsonElement root)
    {
        var payload = GetPayload(root, "mediaProfileNegotiation", "media_profile_negotiation");
        return new ProtocolFrame.MediaProfileNegotiation(
            PreferredProfile: ParseMediaProfile(GetString(payload, "preferredProfile", "preferred_profile")),
            NegotiatedProfile: ParseMediaProfile(GetString(payload, "negotiatedProfile", "negotiated_profile")),
            ColorPrimaries: GetString(payload, "colorPrimaries", "color_primaries") ?? "bt709",
            TransferFunction: GetString(payload, "transferFunction", "transfer_function") ?? "gamma",
            Codec: GetString(payload, "codec") ?? "h264");
    }

    private static ProtocolFrame DecodeRecordingNotice(JsonElement root)
    {
        var payload = GetPayload(root, "recordingNotice", "recording_notice");
        return new ProtocolFrame.RecordingNotice(
            ParticipantId: GetString(payload, "participantId", "participantID", "participant_id") ?? string.Empty,
            State: ParseRecordingState(GetString(payload, "state")),
            Mode: GetString(payload, "mode") ?? "local",
            PolicyBasis: GetString(payload, "policyBasis", "policy_basis") ?? "policy-default",
            IssuedAtMs: GetInt64(payload, 0, "issuedAtMs", "issued_at_ms"),
            IssuedBy: GetString(payload, "issuedBy", "issued_by") ?? "system");
    }

    private static ProtocolFrame DecodeE2eeKeyEpoch(JsonElement root)
    {
        var payload = GetPayload(root, "e2eeKeyEpoch", "e2ee_key_epoch");
        return new ProtocolFrame.E2eeKeyEpoch(
            Epoch: GetInt32(payload, 0, "epoch"),
            IssuedBy: GetString(payload, "issuedBy", "issued_by", "participantId", "participantID", "participant_id") ?? "unknown",
            Signature: GetString(payload, "signature"),
            SentAtMs: GetInt64(payload, 0, "sentAtMs", "sent_at_ms", "issuedAtMs", "issued_at_ms"));
    }

    private static ProtocolFrame DecodeKeyRotationAck(JsonElement root)
    {
        var payload = GetPayload(root, "keyRotationAck", "key_rotation_ack");
        return new ProtocolFrame.KeyRotationAck(
            ParticipantId: GetString(payload, "participantId", "participantID", "participant_id") ?? string.Empty,
            AckEpoch: GetInt32(payload, 0, "ackEpoch", "ack_epoch"),
            ReceivedAtMs: GetInt64(payload, 0, "receivedAtMs", "received_at_ms"));
    }

    private static ProtocolFrame DecodePaymentPolicy(JsonElement root)
    {
        var payload = GetPayload(root, "paymentPolicy", "payment_policy");
        return new ProtocolFrame.PaymentPolicy(
            Required: GetBool(payload, false, "required"),
            DestinationAccount: GetString(payload, "destinationAccount", "destination_account"));
    }

    private static ProtocolFrame DecodePaymentSettlement(JsonElement root)
    {
        var payload = GetPayload(root, "paymentSettlement", "payment_settlement");
        return new ProtocolFrame.PaymentSettlement(
            Status: ParsePaymentSettlementStatus(GetString(payload, "status")));
    }

    private static ProtocolFrame DecodePing(JsonElement root)
    {
        var payload = GetPayload(root, "ping");
        return new ProtocolFrame.Ping(GetInt64(payload, 0, "sentAtMs", "sent_at_ms"));
    }

    private static ProtocolFrame DecodePong(JsonElement root)
    {
        var payload = GetPayload(root, "pong");
        return new ProtocolFrame.Pong(GetInt64(payload, 0, "sentAtMs", "sent_at_ms"));
    }

    private static ProtocolFrame DecodeError(JsonElement root)
    {
        var payload = GetPayload(root, "error");
        return new ProtocolFrame.Error(
            Category: ParseErrorCategory(GetString(payload, "category")),
            Code: GetString(payload, "code") ?? "error",
            Message: GetString(payload, "message") ?? "unknown");
    }

    private static IReadOnlyList<Participant> ParseParticipants(JsonElement element, params string[] keys)
    {
        if (!TryGetPropertyAlias(element, out var array, keys) || array.ValueKind != JsonValueKind.Array)
        {
            return Array.Empty<Participant>();
        }

        var participants = new List<Participant>();
        foreach (var entry in array.EnumerateArray())
        {
            if (entry.ValueKind != JsonValueKind.Object)
            {
                continue;
            }

            var id = GetString(entry, "id");
            if (string.IsNullOrWhiteSpace(id))
            {
                continue;
            }

            participants.Add(new Participant(
                Id: id,
                DisplayName: GetString(entry, "displayName", "display_name") ?? id,
                Role: ParseParticipantRole(GetString(entry, "role")),
                Muted: GetBool(entry, false, "muted"),
                VideoEnabled: GetBool(entry, true, "videoEnabled", "video_enabled"),
                ShareEnabled: GetBool(entry, false, "shareEnabled", "share_enabled"),
                WaitingRoom: GetBool(entry, false, "waitingRoom", "waiting_room")));
        }

        return participants;
    }

    private static IReadOnlyList<RoleChange> ParseRoleChanges(JsonElement element, params string[] keys)
    {
        if (!TryGetPropertyAlias(element, out var array, keys) || array.ValueKind != JsonValueKind.Array)
        {
            return Array.Empty<RoleChange>();
        }

        var changes = new List<RoleChange>();
        foreach (var entry in array.EnumerateArray())
        {
            if (entry.ValueKind != JsonValueKind.Object)
            {
                continue;
            }

            var participantId = GetString(entry, "participantId", "participantID", "participant_id");
            if (string.IsNullOrWhiteSpace(participantId))
            {
                continue;
            }

            changes.Add(new RoleChange(participantId, ParseParticipantRole(GetString(entry, "role"))));
        }

        return changes;
    }

    private static IReadOnlyList<string> ParseStringArray(JsonElement element, params string[] keys)
    {
        if (!TryGetPropertyAlias(element, out var array, keys) || array.ValueKind != JsonValueKind.Array)
        {
            return Array.Empty<string>();
        }

        var values = new List<string>();
        foreach (var entry in array.EnumerateArray())
        {
            if (entry.ValueKind == JsonValueKind.String)
            {
                var value = entry.GetString();
                if (!string.IsNullOrWhiteSpace(value))
                {
                    values.Add(value);
                }
            }
        }

        return values;
    }

    private static JsonElement GetPayload(JsonElement root, params string[] keys)
    {
        if (root.ValueKind != JsonValueKind.Object)
        {
            return root;
        }

        foreach (var key in keys)
        {
            if (root.TryGetProperty(key, out var payload) && payload.ValueKind == JsonValueKind.Object)
            {
                return payload;
            }
        }

        return root;
    }

    private static bool TryGetPropertyAlias(JsonElement element, out JsonElement value, params string[] names)
    {
        value = default;
        if (element.ValueKind != JsonValueKind.Object)
        {
            return false;
        }

        foreach (var name in names)
        {
            if (element.TryGetProperty(name, out value))
            {
                return true;
            }
        }

        return false;
    }

    private static string? GetString(JsonElement element, params string[] names)
    {
        if (!TryGetPropertyAlias(element, out var value, names))
        {
            return null;
        }
        return value.ValueKind == JsonValueKind.String ? value.GetString() : null;
    }

    private static bool GetBool(JsonElement element, bool defaultValue, params string[] names)
    {
        if (!TryGetPropertyAlias(element, out var value, names))
        {
            return defaultValue;
        }
        return value.ValueKind == JsonValueKind.True || value.ValueKind == JsonValueKind.False
            ? value.GetBoolean()
            : defaultValue;
    }

    private static int GetInt32(JsonElement element, int defaultValue, params string[] names)
    {
        if (!TryGetPropertyAlias(element, out var value, names))
        {
            return defaultValue;
        }
        return value.ValueKind == JsonValueKind.Number && value.TryGetInt32(out var parsed)
            ? parsed
            : defaultValue;
    }

    private static long GetInt64(JsonElement element, long defaultValue, params string[] names)
    {
        if (!TryGetPropertyAlias(element, out var value, names))
        {
            return defaultValue;
        }
        return value.ValueKind == JsonValueKind.Number && value.TryGetInt64(out var parsed)
            ? parsed
            : defaultValue;
    }

    private static string NormalizeKind(string rawKind)
    {
        return rawKind switch
        {
            "handshake_ack" => "handshakeAck",
            "participant_presence_delta" => "participantPresenceDelta",
            "role_grant" => "roleGrant",
            "role_revoke" => "roleRevoke",
            "permissions_snapshot" => "permissionsSnapshot",
            "moderation_signed" => "moderationSigned",
            "session_policy" => "sessionPolicy",
            "device_capability" => "deviceCapability",
            "media_profile_negotiation" => "mediaProfileNegotiation",
            "recording_notice" => "recordingNotice",
            "e2ee_key_epoch" => "e2eeKeyEpoch",
            "key_rotation_ack" => "keyRotationAck",
            "payment_policy" => "paymentPolicy",
            "payment_settlement" => "paymentSettlement",
            _ => rawKind
        };
    }

    private static ParticipantRole ParseParticipantRole(string? raw)
    {
        return raw switch
        {
            "host" => ParticipantRole.Host,
            "co_host" or "coHost" => ParticipantRole.CoHost,
            "guest" => ParticipantRole.Guest,
            _ => ParticipantRole.Participant
        };
    }

    private static ModerationAction ParseModerationAction(string? raw)
    {
        return raw switch
        {
            "mute" => ModerationAction.Mute,
            "video_off" or "videoOff" => ModerationAction.VideoOff,
            "stop_share" or "stopShare" => ModerationAction.StopShare,
            "kick" => ModerationAction.Kick,
            "admit_from_waiting" or "admitFromWaiting" => ModerationAction.AdmitFromWaiting,
            "deny_from_waiting" or "denyFromWaiting" => ModerationAction.DenyFromWaiting,
            _ => ModerationAction.Mute
        };
    }

    private static MediaProfile ParseMediaProfile(string? raw)
    {
        return raw == "hdr" ? MediaProfile.Hdr : MediaProfile.Sdr;
    }

    private static RecordingState ParseRecordingState(string? raw)
    {
        return raw == "started" ? RecordingState.Started : RecordingState.Stopped;
    }

    private static GuestPolicy ParseGuestPolicy(string? raw)
    {
        return raw switch
        {
            "invite_only" or "inviteOnly" => GuestPolicy.InviteOnly,
            "blocked" => GuestPolicy.Blocked,
            _ => GuestPolicy.Open
        };
    }

    private static PaymentSettlementStatus ParsePaymentSettlementStatus(string? raw)
    {
        return raw switch
        {
            "pending" => PaymentSettlementStatus.Pending,
            "settled" => PaymentSettlementStatus.Settled,
            "blocked" => PaymentSettlementStatus.Blocked,
            "not_required" or "notRequired" => PaymentSettlementStatus.NotRequired,
            _ => PaymentSettlementStatus.NotRequired
        };
    }

    private static SessionErrorCategory ParseErrorCategory(string? raw)
    {
        return raw switch
        {
            "policyFailure" or "policy_failure" => SessionErrorCategory.PolicyFailure,
            "transportFailure" or "transport_failure" => SessionErrorCategory.TransportFailure,
            _ => SessionErrorCategory.ProtocolFailure
        };
    }

    private static string ToWire(SessionErrorCategory category)
    {
        return category switch
        {
            SessionErrorCategory.PolicyFailure => "policyFailure",
            SessionErrorCategory.TransportFailure => "transportFailure",
            _ => "protocolFailure"
        };
    }

    private static string ToWire(ParticipantRole role)
    {
        return role switch
        {
            ParticipantRole.Host => "host",
            ParticipantRole.CoHost => "co_host",
            ParticipantRole.Guest => "guest",
            _ => "participant"
        };
    }

    private static string ToWire(ModerationAction action)
    {
        return action switch
        {
            ModerationAction.VideoOff => "videoOff",
            ModerationAction.StopShare => "stopShare",
            ModerationAction.Kick => "kick",
            ModerationAction.AdmitFromWaiting => "admitFromWaiting",
            ModerationAction.DenyFromWaiting => "denyFromWaiting",
            _ => "mute"
        };
    }

    private static string ToWire(MediaProfile profile)
    {
        return profile == MediaProfile.Hdr ? "hdr" : "sdr";
    }

    private static string ToWire(RecordingState state)
    {
        return state == RecordingState.Started ? "started" : "stopped";
    }

    private static string ToWire(GuestPolicy policy)
    {
        return policy switch
        {
            GuestPolicy.InviteOnly => "invite_only",
            GuestPolicy.Blocked => "blocked",
            _ => "open"
        };
    }

    private static string ToWire(PaymentSettlementStatus status)
    {
        return status switch
        {
            PaymentSettlementStatus.Pending => "pending",
            PaymentSettlementStatus.Settled => "settled",
            PaymentSettlementStatus.Blocked => "blocked",
            _ => "notRequired"
        };
    }
}

public abstract record RuntimeDirective
{
    public sealed record None : RuntimeDirective;
    public sealed record ReconnectScheduled(int Attempt, long DueAtMs) : RuntimeDirective;
    public sealed record FallbackActivated(string Reason) : RuntimeDirective;
}

public enum MeetingTelemetryCategory
{
    ConnectionLifecycle,
    FallbackLifecycle,
    PolicyFailure
}

public sealed record MeetingTelemetryEvent(
    MeetingTelemetryCategory Category,
    string Name,
    IReadOnlyDictionary<string, string> Attributes,
    long AtMs);

public interface IMeetingTelemetrySink
{
    void Record(MeetingTelemetryEvent telemetryEvent);
}

public sealed class NoOpMeetingTelemetrySink : IMeetingTelemetrySink
{
    public static readonly NoOpMeetingTelemetrySink Instance = new();

    private NoOpMeetingTelemetrySink() { }

    public void Record(MeetingTelemetryEvent telemetryEvent) { }
}

public sealed class SessionRuntime
{
    private readonly IReadOnlyList<long> reconnectBackoffMs;
    private readonly IMeetingTelemetrySink telemetrySink;
    private int reconnectAttempt;
    private long? reconnectDueAtMs;
    private bool userInitiatedDisconnect;
    private bool appInBackground;

    public SessionRuntime(MeetingConfig config)
        : this(config, new long[] { 1_000, 2_000, 4_000, 8_000 }, NoOpMeetingTelemetrySink.Instance)
    {
    }

    public SessionRuntime(MeetingConfig config, IReadOnlyList<long> reconnectBackoffMs)
        : this(config, reconnectBackoffMs, NoOpMeetingTelemetrySink.Instance)
    {
    }

    public SessionRuntime(
        MeetingConfig config,
        IReadOnlyList<long> reconnectBackoffMs,
        IMeetingTelemetrySink telemetrySink)
    {
        State = ProtocolSessionState.Initial(config);
        this.reconnectBackoffMs = reconnectBackoffMs;
        this.telemetrySink = telemetrySink;
        reconnectAttempt = 0;
        reconnectDueAtMs = null;
        userInitiatedDisconnect = false;
        appInBackground = false;
    }

    public ProtocolSessionState State { get; private set; }

    public long? ReconnectDueAtMs => reconnectDueAtMs;

    public void ConnectRequested(long nowMs)
    {
        RequestConnect(resetBackoff: true, source: "manual", nowMs);
    }

    public void OnAppForegrounded(long nowMs)
    {
        if (userInitiatedDisconnect)
        {
            return;
        }

        appInBackground = false;
        RecordConnectionEvent("app_foregrounded", EmptyAttributes(), nowMs);
        var phase = State.ConnectionPhase;
        if (phase == ConnectionPhase.Disconnected || phase == ConnectionPhase.Degraded || phase == ConnectionPhase.Error)
        {
            RequestConnect(resetBackoff: false, source: "foreground", nowMs);
        }
    }

    public void OnAppBackgrounded(long nowMs)
    {
        appInBackground = true;
        reconnectDueAtMs = null;
        RecordConnectionEvent("app_backgrounded", EmptyAttributes(), nowMs);
        if (userInitiatedDisconnect)
        {
            return;
        }

        var phase = State.ConnectionPhase;
        if (phase == ConnectionPhase.Connected || phase == ConnectionPhase.Connecting || phase == ConnectionPhase.Degraded)
        {
            ApplyEvent(new ProtocolEvent.TransportDisconnected("app_backgrounded"), nowMs);
        }
    }

    public void OnConnectivityChanged(bool available, long nowMs)
    {
        if (userInitiatedDisconnect)
        {
            return;
        }

        if (available)
        {
            RecordConnectionEvent("network_available", EmptyAttributes(), nowMs);
            if (appInBackground)
            {
                RecordConnectionEvent("connectivity_restore_deferred_backgrounded", EmptyAttributes(), nowMs);
                return;
            }

            if (State.ConnectionPhase != ConnectionPhase.Connected &&
                State.ConnectionPhase != ConnectionPhase.Connecting &&
                !State.Fallback.Active)
            {
                RequestConnect(resetBackoff: false, source: "connectivity_restore", nowMs);
            }

            return;
        }

        RecordConnectionEvent("network_unavailable", EmptyAttributes(), nowMs);
        ApplyEvent(new ProtocolEvent.TransportFailure("network_unavailable"), nowMs);
        if (appInBackground)
        {
            RecordConnectionEvent(
                "reconnect_deferred_backgrounded",
                new Dictionary<string, string> { ["trigger"] = "network_unavailable" },
                nowMs);
            return;
        }

        _ = ScheduleReconnectOrFallback("network_unavailable", nowMs);
    }

    public void OnAudioInterruptionBegan(long nowMs)
    {
        if (userInitiatedDisconnect)
        {
            return;
        }

        RecordConnectionEvent("audio_interruption_began", EmptyAttributes(), nowMs);
        ApplyEvent(new ProtocolEvent.TransportFailure("audio_interruption"), nowMs);
        if (appInBackground)
        {
            RecordConnectionEvent(
                "reconnect_deferred_backgrounded",
                new Dictionary<string, string> { ["trigger"] = "audio_interruption" },
                nowMs);
            return;
        }

        _ = ScheduleReconnectOrFallback("audio_interruption", nowMs);
    }

    public void OnAudioInterruptionEnded(bool shouldReconnect, long nowMs)
    {
        if (userInitiatedDisconnect)
        {
            return;
        }

        RecordConnectionEvent(
            "audio_interruption_ended",
            new Dictionary<string, string>
            {
                ["should_reconnect"] = shouldReconnect ? "true" : "false"
            },
            nowMs);

        if (!shouldReconnect || appInBackground || State.Fallback.Active)
        {
            return;
        }

        if (State.ConnectionPhase != ConnectionPhase.Connected &&
            State.ConnectionPhase != ConnectionPhase.Connecting)
        {
            RequestConnect(resetBackoff: false, source: "audio_interruption_end", nowMs);
        }
    }

    public void OnAudioRouteChanged(string reason, long nowMs)
    {
        RecordConnectionEvent(
            "audio_route_changed",
            new Dictionary<string, string> { ["reason"] = reason },
            nowMs);
    }

    public IReadOnlyList<ProtocolFrame> OnTransportConnected(long nowMs)
    {
        if (appInBackground)
        {
            reconnectDueAtMs = null;
            RecordConnectionEvent("transport_connected_while_backgrounded", EmptyAttributes(), nowMs);
            ApplyEvent(new ProtocolEvent.TransportDisconnected("backgrounded_before_handshake"), nowMs);
            return Array.Empty<ProtocolFrame>();
        }

        userInitiatedDisconnect = false;
        reconnectAttempt = 0;
        reconnectDueAtMs = null;
        RecordConnectionEvent("transport_connected", EmptyAttributes(), nowMs);
        ApplyEvent(new ProtocolEvent.TransportConnected(), nowMs);
        return HandshakeFrames(nowMs);
    }

    public RuntimeDirective OnTransportDisconnected(string reason, long nowMs)
    {
        RecordConnectionEvent(
            "transport_disconnected",
            new Dictionary<string, string> { ["reason"] = reason },
            nowMs);
        ApplyEvent(new ProtocolEvent.TransportDisconnected(reason), nowMs);
        if (appInBackground)
        {
            RecordConnectionEvent(
                "reconnect_deferred_backgrounded",
                new Dictionary<string, string> { ["trigger"] = reason },
                nowMs);
            return new RuntimeDirective.None();
        }
        return ScheduleReconnectOrFallback(reason, nowMs);
    }

    public RuntimeDirective OnTransportFailure(string message, long nowMs)
    {
        RecordConnectionEvent(
            "transport_failure",
            new Dictionary<string, string> { ["message"] = message },
            nowMs);
        ApplyEvent(new ProtocolEvent.TransportFailure(message), nowMs);
        if (appInBackground)
        {
            RecordConnectionEvent(
                "reconnect_deferred_backgrounded",
                new Dictionary<string, string> { ["trigger"] = message },
                nowMs);
            return new RuntimeDirective.None();
        }
        return ScheduleReconnectOrFallback(message, nowMs);
    }

    public void OnSendFailure(string message, long nowMs)
    {
        RecordConnectionEvent(
            "send_failure",
            new Dictionary<string, string> { ["message"] = message },
            nowMs);
        ApplyEvent(new ProtocolEvent.FrameSendFailed(message), nowMs);
    }

    public void OnManualDisconnect(long nowMs)
    {
        userInitiatedDisconnect = true;
        reconnectDueAtMs = null;
        RecordConnectionEvent("manual_disconnect", EmptyAttributes(), nowMs);
        ApplyEvent(new ProtocolEvent.ManualDisconnected(), nowMs);
    }

    public IReadOnlyList<ProtocolFrame> OnFrame(ProtocolFrame frame, long nowMs)
    {
        var shouldPong = frame is ProtocolFrame.Ping;
        var epochFrame = frame as ProtocolFrame.E2eeKeyEpoch;
        var e2eeSignatureValid = epochFrame is not null && HasRequiredSignature(epochFrame.Signature, State.Config);
        var participantId = ResolveParticipantId(State.Config);
        ApplyEvent(new ProtocolEvent.FrameReceived(frame), nowMs);
        var outbound = new List<ProtocolFrame>();

        if (shouldPong)
        {
            outbound.Add(new ProtocolFrame.Pong(nowMs));
        }

        if (epochFrame is not null && e2eeSignatureValid && State.E2eeState.CurrentEpoch >= epochFrame.Epoch)
        {
            var ack = new ProtocolFrame.KeyRotationAck(
                ParticipantId: participantId,
                AckEpoch: epochFrame.Epoch,
                ReceivedAtMs: nowMs);
            outbound.Add(ack);
            ApplyEvent(new ProtocolEvent.FrameReceived(ack), nowMs);
        }

        return outbound.Count == 0 ? Array.Empty<ProtocolFrame>() : outbound;
    }

    public void RecoverFromFallback(long nowMs)
    {
        userInitiatedDisconnect = false;
        reconnectAttempt = 0;
        reconnectDueAtMs = null;
        RecordConnectionEvent("fallback_recovery_requested", EmptyAttributes(), nowMs);
        ApplyEvent(new ProtocolEvent.FallbackRecovered(), nowMs);
        ApplyEvent(new ProtocolEvent.ConnectRequested(), nowMs);
    }

    public bool TakeReconnectIfDue(long nowMs)
    {
        if (!reconnectDueAtMs.HasValue)
        {
            return false;
        }
        if (nowMs < reconnectDueAtMs.Value || userInitiatedDisconnect || State.Fallback.Active || appInBackground)
        {
            return false;
        }

        reconnectDueAtMs = null;
        RecordConnectionEvent("reconnect_attempt", EmptyAttributes(), nowMs);
        RequestConnect(resetBackoff: false, source: "reconnect", nowMs);
        return true;
    }

    private RuntimeDirective ScheduleReconnectOrFallback(string trigger, long nowMs)
    {
        if (userInitiatedDisconnect || State.Fallback.Active || appInBackground)
        {
            return new RuntimeDirective.None();
        }
        if (reconnectDueAtMs.HasValue)
        {
            return new RuntimeDirective.None();
        }

        if (reconnectAttempt >= reconnectBackoffMs.Count)
        {
            var reason = $"Reconnect exhausted after {reconnectAttempt} attempts: {trigger}";
            State = ProtocolReducer.Reduce(State, new ProtocolEvent.FallbackActivated(reason), nowMs);
            return new RuntimeDirective.FallbackActivated(reason);
        }

        var delayMs = Math.Max(0, reconnectBackoffMs[reconnectAttempt]);
        reconnectAttempt += 1;
        var dueAtMs = nowMs + delayMs;
        reconnectDueAtMs = dueAtMs;
        RecordConnectionEvent(
            "reconnect_scheduled",
            new Dictionary<string, string>
            {
                ["attempt"] = reconnectAttempt.ToString(),
                ["due_at_ms"] = dueAtMs.ToString(),
                ["trigger"] = trigger
            },
            nowMs);
        return new RuntimeDirective.ReconnectScheduled(reconnectAttempt, dueAtMs);
    }

    private void RequestConnect(bool resetBackoff, string source, long nowMs)
    {
        if (appInBackground)
        {
            RecordConnectionEvent(
                "connect_deferred_backgrounded",
                new Dictionary<string, string> { ["source"] = source },
                nowMs);
            return;
        }

        userInitiatedDisconnect = false;
        reconnectDueAtMs = null;
        if (resetBackoff)
        {
            reconnectAttempt = 0;
        }

        RecordConnectionEvent(
            "connect_requested",
            new Dictionary<string, string> { ["source"] = source },
            nowMs);
        ApplyEvent(new ProtocolEvent.ConnectRequested(), nowMs);
    }

    private IReadOnlyList<ProtocolFrame> HandshakeFrames(long nowMs)
    {
        var participantId = ResolveParticipantId(State.Config);
        var preferredProfile = State.Config.SupportsHdrCapture && State.Config.SupportsHdrRender
            ? MediaProfile.Hdr
            : MediaProfile.Sdr;

        var frames = new List<ProtocolFrame>
        {
            new ProtocolFrame.Handshake(
                RoomId: State.Config.RoomId,
                ParticipantId: participantId,
                ParticipantName: State.Config.ParticipantName,
                WalletIdentity: State.Config.WalletIdentity,
                ResumeToken: State.ResumeToken,
                PreferredProfile: preferredProfile,
                HdrCapture: State.Config.SupportsHdrCapture,
                HdrRender: State.Config.SupportsHdrRender,
                SentAtMs: nowMs),
            new ProtocolFrame.DeviceCapability(
                ParticipantId: participantId,
                Codecs: new[] { "h264", "vp9" },
                HdrCapture: State.Config.SupportsHdrCapture,
                HdrRender: State.Config.SupportsHdrRender,
                MaxStreams: 4,
                UpdatedAtMs: nowMs)
        };

        if (State.Config.RequirePaymentSettlement)
        {
            frames.Add(new ProtocolFrame.PaymentPolicy(
                Required: true,
                DestinationAccount: "nexus://payment-policy"));
        }

        return frames;
    }

    private static string ResolveParticipantId(MeetingConfig config)
    {
        if (!string.IsNullOrWhiteSpace(config.ParticipantId))
        {
            return NormalizeParticipantId(config.ParticipantId);
        }

        return NormalizeParticipantId(config.ParticipantName);
    }

    private static string NormalizeParticipantId(string? raw)
    {
        var source = string.IsNullOrWhiteSpace(raw) ? "participant" : raw.Trim();
        var normalized = new StringBuilder(source.Length);

        foreach (var ch in source.ToLowerInvariant())
        {
            if ((ch >= 'a' && ch <= 'z') || (ch >= '0' && ch <= '9') || ch == '-' || ch == '_')
            {
                normalized.Append(ch);
            }
            else if (char.IsWhiteSpace(ch))
            {
                normalized.Append('-');
            }
        }

        return normalized.Length == 0 ? "participant" : normalized.ToString();
    }

    private static bool HasRequiredSignature(string? signature, MeetingConfig config)
    {
        if (!config.RequireSignedModeration)
        {
            return true;
        }
        return !string.IsNullOrWhiteSpace(signature);
    }

    private void ApplyEvent(ProtocolEvent protocolEvent, long nowMs)
    {
        var previous = State;
        State = ProtocolReducer.Reduce(State, protocolEvent, nowMs);
        EmitStateTransitionTelemetry(previous, State, nowMs);
    }

    private void EmitStateTransitionTelemetry(ProtocolSessionState previous, ProtocolSessionState next, long nowMs)
    {
        if (previous.ConnectionPhase != next.ConnectionPhase)
        {
            RecordConnectionEvent(
                "phase_changed",
                new Dictionary<string, string>
                {
                    ["from"] = previous.ConnectionPhase.ToString(),
                    ["to"] = next.ConnectionPhase.ToString()
                },
                nowMs);
        }

        if (!previous.Fallback.Active && next.Fallback.Active)
        {
            RecordFallbackEvent(
                "fallback_activated",
                new Dictionary<string, string>
                {
                    ["reason"] = next.Fallback.Reason ?? "unknown"
                },
                nowMs);
        }

        if (previous.Fallback.Active && !next.Fallback.Active)
        {
            var attributes = new Dictionary<string, string>();
            if (next.Fallback.LastRtoMs.HasValue)
            {
                attributes["rto_ms"] = next.Fallback.LastRtoMs.Value.ToString();
            }
            RecordFallbackEvent("fallback_recovered", attributes, nowMs);
        }

        if (!Equals(previous.LastError, next.LastError) && next.LastError?.Category == SessionErrorCategory.PolicyFailure)
        {
            RecordPolicyFailureEvent(
                next.LastError.Code,
                new Dictionary<string, string>
                {
                    ["code"] = next.LastError.Code,
                    ["message"] = next.LastError.Message
                },
                nowMs);
        }
    }

    private void RecordConnectionEvent(string name, IReadOnlyDictionary<string, string> attributes, long nowMs)
    {
        RecordTelemetryEvent(MeetingTelemetryCategory.ConnectionLifecycle, name, attributes, nowMs);
    }

    private void RecordFallbackEvent(string name, IReadOnlyDictionary<string, string> attributes, long nowMs)
    {
        RecordTelemetryEvent(MeetingTelemetryCategory.FallbackLifecycle, name, attributes, nowMs);
    }

    private void RecordPolicyFailureEvent(string name, IReadOnlyDictionary<string, string> attributes, long nowMs)
    {
        RecordTelemetryEvent(MeetingTelemetryCategory.PolicyFailure, name, attributes, nowMs);
    }

    private void RecordTelemetryEvent(
        MeetingTelemetryCategory category,
        string name,
        IReadOnlyDictionary<string, string> attributes,
        long nowMs)
    {
        telemetrySink.Record(new MeetingTelemetryEvent(category, name, attributes, nowMs));
    }

    private static IReadOnlyDictionary<string, string> EmptyAttributes()
    {
        return new Dictionary<string, string>();
    }
}
