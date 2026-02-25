import { decodeFrame, encodeFrame } from './codec';
import type { ProtocolFrame } from './types';

export type TransportEvent =
  | { kind: 'connected' }
  | { kind: 'disconnected'; reason: string }
  | { kind: 'failure'; message: string }
  | { kind: 'frame'; frame: ProtocolFrame };

export interface ProtocolTransport {
  connect(url: string): void;
  disconnect(): void;
  send(frame: ProtocolFrame): void;
  onEvent(handler: (event: TransportEvent) => void): void;
}

export class BrowserWebSocketTransport implements ProtocolTransport {
  private ws?: WebSocket;
  private handler?: (event: TransportEvent) => void;

  onEvent(handler: (event: TransportEvent) => void): void {
    this.handler = handler;
  }

  connect(url: string): void {
    this.disconnect();
    try {
      this.ws = new WebSocket(url);
    } catch (error) {
      this.emit({
        kind: 'failure',
        message: error instanceof Error ? error.message : 'Failed to initialize WebSocket'
      });
      return;
    }

    this.ws.onopen = () => this.emit({ kind: 'connected' });
    this.ws.onclose = (event) => this.emit({ kind: 'disconnected', reason: event.reason || 'socket_closed' });
    this.ws.onerror = () => this.emit({ kind: 'failure', message: 'websocket_error' });
    this.ws.onmessage = (message) => {
      try {
        this.emit({ kind: 'frame', frame: decodeFrame(String(message.data)) });
      } catch (error) {
        this.emit({
          kind: 'failure',
          message: error instanceof Error ? `decode_error:${error.message}` : 'decode_error'
        });
      }
    };
  }

  disconnect(): void {
    if (!this.ws) return;
    this.ws.onopen = null;
    this.ws.onclose = null;
    this.ws.onerror = null;
    this.ws.onmessage = null;
    try {
      this.ws.close();
    } finally {
      this.ws = undefined;
    }
  }

  send(frame: ProtocolFrame): void {
    if (!this.ws || this.ws.readyState !== WebSocket.OPEN) {
      this.emit({ kind: 'failure', message: 'transport_not_connected' });
      return;
    }
    try {
      this.ws.send(encodeFrame(frame));
    } catch (error) {
      this.emit({
        kind: 'failure',
        message: error instanceof Error ? error.message : 'transport_send_failed'
      });
    }
  }

  private emit(event: TransportEvent): void {
    this.handler?.(event);
  }
}

export class MemoryTransport implements ProtocolTransport {
  private handler?: (event: TransportEvent) => void;
  private connected = false;

  onEvent(handler: (event: TransportEvent) => void): void {
    this.handler = handler;
  }

  connect(_url: string): void {
    this.connected = true;
    this.handler?.({ kind: 'connected' });
  }

  disconnect(): void {
    if (!this.connected) return;
    this.connected = false;
    this.handler?.({ kind: 'disconnected', reason: 'memory_disconnect' });
  }

  send(frame: ProtocolFrame): void {
    if (!this.connected) {
      this.handler?.({ kind: 'failure', message: 'memory_transport_not_connected' });
      return;
    }
    this.handler?.({ kind: 'frame', frame });
  }
}
