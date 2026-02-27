import SwiftUI
#if canImport(WebKit)
import WebKit
#endif

struct WebFallbackView: View {
    let url: URL

    var body: some View {
#if canImport(WebKit) && os(macOS)
        WebView(url: url)
            .navigationTitle("Web Fallback")
            .accessibilityIdentifier("kaigi.fallback.webview")
#elseif canImport(WebKit) && canImport(UIKit)
        WebView(url: url)
            .navigationTitle("Web Fallback")
            .navigationBarTitleDisplayMode(.inline)
            .accessibilityIdentifier("kaigi.fallback.webview")
#else
        VStack(spacing: 10) {
            Text("Web Fallback Unsupported")
                .font(.headline)
            Text("This platform does not support embedded web fallback.")
                .font(.footnote)
                .multilineTextAlignment(.center)
        }
        .padding()
        .accessibilityIdentifier("kaigi.fallback.unsupported")
#endif
    }
}

#if canImport(WebKit) && os(macOS)
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
#endif

#if canImport(WebKit) && canImport(UIKit)
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
