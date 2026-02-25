export enum MeetingTelemetryCategory {
  ConnectionLifecycle = 'connection_lifecycle',
  FallbackLifecycle = 'fallback_lifecycle',
  PolicyFailure = 'policy_failure'
}

export interface MeetingTelemetryEvent {
  category: MeetingTelemetryCategory;
  name: string;
  attributes: Record<string, string>;
  atMs: number;
}

export interface MeetingTelemetrySink {
  record: (event: MeetingTelemetryEvent) => void;
}

export const NoOpMeetingTelemetrySink: MeetingTelemetrySink = {
  record: () => {}
};
