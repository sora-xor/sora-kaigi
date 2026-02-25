export enum ConnectionPhase {
  Disconnected = 'Disconnected',
  Connecting = 'Connecting',
  Connected = 'Connected',
  Degraded = 'Degraded',
  FallbackActive = 'FallbackActive',
  Error = 'Error'
}

export enum SessionErrorCategory {
  ProtocolFailure = 'protocol_failure',
  PolicyFailure = 'policy_failure',
  TransportFailure = 'transport_failure'
}

export interface SessionError {
  category: SessionErrorCategory;
  code: string;
  message: string;
  atMs: number;
}

export enum ParticipantRole {
  Host = 'host',
  CoHost = 'co_host',
  Participant = 'participant',
  Guest = 'guest'
}

export interface Participant {
  id: string;
  displayName: string;
  role: ParticipantRole;
  muted: boolean;
  videoEnabled: boolean;
  shareEnabled: boolean;
  waitingRoom: boolean;
}

export interface RoleChange {
  participantId: string;
  role: ParticipantRole;
}

export enum ModerationAction {
  Mute = 'mute',
  VideoOff = 'video_off',
  StopShare = 'stop_share',
  Kick = 'kick',
  AdmitFromWaiting = 'admit_from_waiting',
  DenyFromWaiting = 'deny_from_waiting'
}

export enum MediaProfile {
  SDR = 'sdr',
  HDR = 'hdr'
}

export interface MediaProfileState {
  preferredProfile: MediaProfile;
  negotiatedProfile: MediaProfile;
  colorPrimaries: string;
  transferFunction: string;
  codec: string;
}

export enum RecordingState {
  Stopped = 'stopped',
  Started = 'started'
}

export enum GuestPolicy {
  Open = 'open',
  InviteOnly = 'invite_only',
  Blocked = 'blocked'
}

export interface PermissionSnapshot {
  effectivePermissions: string[];
  epoch: number;
}

export enum PaymentSettlementStatus {
  NotRequired = 'not_required',
  Pending = 'pending',
  Settled = 'settled',
  Blocked = 'blocked'
}

export interface PaymentState {
  required: boolean;
  destination?: string;
  settlementStatus: PaymentSettlementStatus;
}

export interface E2eeState {
  currentEpoch: number;
  lastAckEpoch: number;
}

export interface FallbackState {
  active: boolean;
  reason?: string;
  activatedAtMs?: number;
  recoveredAtMs?: number;
  lastRtoMs?: number;
}

export interface MeetingConfig {
  signalingUrl: string;
  fallbackUrl: string;
  roomId: string;
  participantId: string;
  participantName: string;
  walletIdentity?: string;
  requireSignedModeration: boolean;
  requirePaymentSettlement: boolean;
  preferWebFallbackOnPolicyFailure: boolean;
  supportsHdrCapture: boolean;
  supportsHdrRender: boolean;
}

export interface ProtocolSessionState {
  config: MeetingConfig;
  connectionPhase: ConnectionPhase;
  handshakeComplete: boolean;
  resumeToken?: string;
  participants: Record<string, Participant>;
  permissionSnapshots: Record<string, PermissionSnapshot>;
  presenceSequence: number;
  roomLocked: boolean;
  waitingRoomEnabled: boolean;
  guestPolicy: GuestPolicy;
  e2eeRequired: boolean;
  maxParticipants: number;
  policyEpoch: number;
  mediaProfile: MediaProfileState;
  recordingNotice: RecordingState;
  e2eeState: E2eeState;
  paymentState: PaymentState;
  fallback: FallbackState;
  lastError?: SessionError;
}

export interface HandshakeFrame {
  kind: 'handshake';
  roomId: string;
  participantId: string;
  participantName: string;
  walletIdentity?: string;
  resumeToken?: string;
  preferredProfile: MediaProfile;
  hdrCapture: boolean;
  hdrRender: boolean;
  sentAtMs: number;
}

export interface HandshakeAckFrame {
  kind: 'handshakeAck';
  sessionId: string;
  resumeToken: string;
  acceptedAtMs: number;
}

export interface PresenceDeltaFrame {
  kind: 'participantPresenceDelta';
  joined: Participant[];
  left: string[];
  roleChanges: RoleChange[];
  sequence: number;
}

export interface RoleGrantFrame {
  kind: 'roleGrant';
  targetParticipantId: string;
  role: ParticipantRole;
  grantedBy: string;
  signature?: string;
  issuedAtMs: number;
}

export interface RoleRevokeFrame {
  kind: 'roleRevoke';
  targetParticipantId: string;
  role: ParticipantRole;
  revokedBy: string;
  signature?: string;
  issuedAtMs: number;
}

export interface PermissionsSnapshotFrame {
  kind: 'permissionsSnapshot';
  participantId: string;
  effectivePermissions: string[];
  epoch: number;
}

export interface ModerationSignedFrame {
  kind: 'moderationSigned';
  targetParticipantId: string;
  action: ModerationAction;
  issuedBy: string;
  signature?: string;
  sentAtMs: number;
}

export interface SessionPolicyFrame {
  kind: 'sessionPolicy';
  roomLock: boolean;
  waitingRoomEnabled: boolean;
  recordingPolicy: RecordingState;
  guestPolicy: GuestPolicy;
  e2eeRequired: boolean;
  maxParticipants: number;
  policyEpoch: number;
  updatedBy: string;
  signature?: string;
  updatedAtMs: number;
}

export interface DeviceCapabilityFrame {
  kind: 'deviceCapability';
  participantId: string;
  codecs: string[];
  hdrCapture: boolean;
  hdrRender: boolean;
  maxStreams: number;
  updatedAtMs: number;
}

export interface MediaProfileNegotiationFrame {
  kind: 'mediaProfileNegotiation';
  preferredProfile: MediaProfile;
  negotiatedProfile: MediaProfile;
  colorPrimaries: string;
  transferFunction: string;
  codec: string;
}

export interface RecordingNoticeFrame {
  kind: 'recordingNotice';
  state: RecordingState;
  issuedAtMs: number;
}

export interface E2eeKeyEpochFrame {
  kind: 'e2eeKeyEpoch';
  epoch: number;
  issuedBy: string;
  signature?: string;
  sentAtMs: number;
}

export interface KeyRotationAckFrame {
  kind: 'keyRotationAck';
  ackEpoch: number;
  participantId: string;
  sentAtMs: number;
}

export interface PaymentPolicyFrame {
  kind: 'paymentPolicy';
  required: boolean;
  destinationAccount?: string;
}

export interface PaymentSettlementFrame {
  kind: 'paymentSettlement';
  status: PaymentSettlementStatus;
}

export interface ErrorFrame {
  kind: 'error';
  category: SessionErrorCategory;
  code: string;
  message: string;
}

export interface PingFrame {
  kind: 'ping';
  sentAtMs: number;
}

export interface PongFrame {
  kind: 'pong';
  sentAtMs: number;
}

export type ProtocolFrame =
  | HandshakeFrame
  | HandshakeAckFrame
  | PresenceDeltaFrame
  | RoleGrantFrame
  | RoleRevokeFrame
  | PermissionsSnapshotFrame
  | ModerationSignedFrame
  | SessionPolicyFrame
  | DeviceCapabilityFrame
  | MediaProfileNegotiationFrame
  | RecordingNoticeFrame
  | E2eeKeyEpochFrame
  | KeyRotationAckFrame
  | PaymentPolicyFrame
  | PaymentSettlementFrame
  | ErrorFrame
  | PingFrame
  | PongFrame;

export type ProtocolEvent =
  | { kind: 'connectRequested' }
  | { kind: 'transportConnected' }
  | { kind: 'transportDisconnected'; reason: string }
  | { kind: 'transportFailure'; message: string }
  | { kind: 'frameReceived'; frame: ProtocolFrame }
  | { kind: 'frameSendFailed'; message: string }
  | { kind: 'manualDisconnected' }
  | { kind: 'fallbackActivated'; reason: string }
  | { kind: 'fallbackRecovered' }
  | { kind: 'configUpdated'; config: MeetingConfig };

export const defaultMediaProfileState: MediaProfileState = {
  preferredProfile: MediaProfile.SDR,
  negotiatedProfile: MediaProfile.SDR,
  colorPrimaries: 'bt709',
  transferFunction: 'gamma',
  codec: 'h264'
};

export const defaultPaymentState: PaymentState = {
  required: false,
  settlementStatus: PaymentSettlementStatus.NotRequired
};

export const defaultFallbackState: FallbackState = {
  active: false
};

export function initialState(config: MeetingConfig): ProtocolSessionState {
  return {
    config,
    connectionPhase: ConnectionPhase.Disconnected,
    handshakeComplete: false,
    participants: {},
    permissionSnapshots: {},
    presenceSequence: 0,
    roomLocked: false,
    waitingRoomEnabled: false,
    guestPolicy: GuestPolicy.Open,
    e2eeRequired: true,
    maxParticipants: 300,
    policyEpoch: 0,
    mediaProfile: defaultMediaProfileState,
    recordingNotice: RecordingState.Stopped,
    e2eeState: { currentEpoch: 0, lastAckEpoch: 0 },
    paymentState: defaultPaymentState,
    fallback: defaultFallbackState
  };
}
