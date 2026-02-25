import SwiftUI
import WebKit

struct WebFallbackView: View {
    let url: URL

    var body: some View {
#if os(macOS)
        WebView(url: url)
            .navigationTitle("Web Fallback")
#else
        WebView(url: url)
            .navigationTitle("Web Fallback")
            .navigationBarTitleDisplayMode(.inline)
#endif
    }
}

#if os(macOS)
private struct WebView: NSViewRepresentable {
    let url: URL

    func makeNSView(context: Context) -> WKWebView {
        let view = WKWebView()
        view.load(URLRequest(url: url))
        return view
    }

    func updateNSView(_ nsView: WKWebView, context: Context) {
        if nsView.url != url {
            nsView.load(URLRequest(url: url))
        }
    }
}
#else
private struct WebView: UIViewRepresentable {
    let url: URL

    func makeUIView(context: Context) -> WKWebView {
        let view = WKWebView()
        view.load(URLRequest(url: url))
        return view
    }

    func updateUIView(_ uiView: WKWebView, context: Context) {
        if uiView.url != url {
            uiView.load(URLRequest(url: url))
        }
    }
}
#endif
