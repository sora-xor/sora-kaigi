import Foundation

enum MeetingProtocolClientEvent {
    case connected
    case disconnected(reason: String)
    case frameReceived(MeetingProtocolFrame)
    case rawTextReceived(String)
    case sendFailed(String)
    case transportFailed(String)
}

protocol MeetingProtocolClient: AnyObject {
    var eventHandler: ((MeetingProtocolClientEvent) -> Void)? { get set }
    func connect(to url: URL)
    func disconnect(reason: String)
    func send(frame: MeetingProtocolFrame)
}

final class URLSessionMeetingProtocolClient: MeetingProtocolClient {
    var eventHandler: ((MeetingProtocolClientEvent) -> Void)?

    private let session: URLSession
    private var socketTask: URLSessionWebSocketTask?
    private var receiveTask: Task<Void, Never>?

    init(session: URLSession = URLSession(configuration: .ephemeral)) {
        self.session = session
    }

    func connect(to url: URL) {
        tearDownSocket(emitEvent: false, reason: "reconnect")

        let task = session.webSocketTask(with: url)
        socketTask = task
        task.resume()
        eventHandler?(.connected)
        startReceiveLoop(task)
    }

    func disconnect(reason: String) {
        tearDownSocket(emitEvent: true, reason: reason)
    }

    private func tearDownSocket(emitEvent: Bool, reason: String) {
        receiveTask?.cancel()
        receiveTask = nil

        if let socketTask {
            socketTask.cancel(with: .goingAway, reason: nil)
            self.socketTask = nil
            if emitEvent {
                eventHandler?(.disconnected(reason: reason))
            }
        }
    }

    func send(frame: MeetingProtocolFrame) {
        guard let socketTask else {
            eventHandler?(.sendFailed("Socket unavailable"))
            return
        }

        Task {
            do {
                let payload = try MeetingProtocolCodec.encode(frame)
                try await socketTask.send(.string(payload))
            } catch {
                eventHandler?(.sendFailed(error.localizedDescription))
            }
        }
    }

    private func startReceiveLoop(_ task: URLSessionWebSocketTask) {
        receiveTask?.cancel()
        receiveTask = Task { [weak self] in
            guard let self else { return }

            while !Task.isCancelled {
                do {
                    let message = try await task.receive()
                    switch message {
                    case .string(let text):
                        do {
                            let frame = try MeetingProtocolCodec.decode(text)
                            eventHandler?(.frameReceived(frame))
                        } catch {
                            eventHandler?(.rawTextReceived(text))
                        }
                    case .data(let data):
                        let text = "binary(\(data.count) bytes)"
                        eventHandler?(.rawTextReceived(text))
                    @unknown default:
                        eventHandler?(.rawTextReceived("unknown payload"))
                    }
                } catch {
                    if Task.isCancelled {
                        break
                    }
                    eventHandler?(.transportFailed(error.localizedDescription))
                    break
                }
            }
        }
    }
}
