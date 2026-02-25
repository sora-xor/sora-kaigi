package io.sora.kaigi.android

import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener

sealed interface TransportEvent {
    data object Connected : TransportEvent
    data class Disconnected(val reason: String) : TransportEvent
    data class FrameReceived(val frame: ProtocolFrame) : TransportEvent
    data class RawMessage(val message: String) : TransportEvent
    data class SendFailed(val message: String) : TransportEvent
    data class Failed(val message: String) : TransportEvent
}

interface ProtocolTransport {
    interface Listener {
        fun onTransportEvent(event: TransportEvent)
    }

    fun connect(config: MeetingConfig, listener: Listener)
    fun disconnect(reason: String = "client_disconnect")
    fun send(frame: ProtocolFrame): Boolean
    fun shutdown()
}

class OkHttpProtocolTransport(
    private val client: OkHttpClient = OkHttpClient()
) : ProtocolTransport {

    private var socket: WebSocket? = null
    private var listener: ProtocolTransport.Listener? = null

    override fun connect(config: MeetingConfig, listener: ProtocolTransport.Listener) {
        val url = config.signalingUriOrNull()?.toString()
        if (url == null) {
            listener.onTransportEvent(TransportEvent.Failed("Invalid signaling URL"))
            return
        }

        this.listener = listener
        disconnectInternal(reason = "reconnect", emitEvent = false)

        val request = Request.Builder().url(url).build()
        socket = client.newWebSocket(request, object : WebSocketListener() {
            override fun onOpen(webSocket: WebSocket, response: Response) {
                this@OkHttpProtocolTransport.listener?.onTransportEvent(TransportEvent.Connected)
            }

            override fun onMessage(webSocket: WebSocket, text: String) {
                val frame = runCatching { ProtocolFrameCodec.decode(text) }.getOrNull()
                if (frame != null) {
                    this@OkHttpProtocolTransport.listener?.onTransportEvent(TransportEvent.FrameReceived(frame))
                } else {
                    this@OkHttpProtocolTransport.listener?.onTransportEvent(TransportEvent.RawMessage(text))
                }
            }

            override fun onClosing(webSocket: WebSocket, code: Int, reason: String) {
                webSocket.close(code, reason)
            }

            override fun onClosed(webSocket: WebSocket, code: Int, reason: String) {
                this@OkHttpProtocolTransport.listener?.onTransportEvent(
                    TransportEvent.Disconnected("$code:$reason")
                )
            }

            override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) {
                this@OkHttpProtocolTransport.listener?.onTransportEvent(
                    TransportEvent.Failed(t.message ?: "unknown transport failure")
                )
            }
        })
    }

    override fun disconnect(reason: String) {
        disconnectInternal(reason = reason, emitEvent = true)
    }

    private fun disconnectInternal(reason: String, emitEvent: Boolean) {
        val active = socket
        socket = null
        if (active != null) {
            active.close(1000, reason)
            if (emitEvent) {
                listener?.onTransportEvent(TransportEvent.Disconnected(reason))
            }
        }
    }

    override fun send(frame: ProtocolFrame): Boolean {
        val active = socket ?: return false
        val payload = runCatching { ProtocolFrameCodec.encode(frame) }
            .getOrElse {
                listener?.onTransportEvent(TransportEvent.SendFailed(it.message ?: "encode failure"))
                return false
            }

        val sent = active.send(payload)
        if (!sent) {
            listener?.onTransportEvent(TransportEvent.SendFailed("Socket rejected send"))
        }
        return sent
    }

    override fun shutdown() {
        disconnect(reason = "shutdown")
        client.dispatcher.executorService.shutdown()
    }
}
