import SwiftUI

struct AuthView: View {
    @EnvironmentObject var app: AppState
    @EnvironmentObject var settings: SettingsStore
    @State private var registering = false
    @State private var email = ""
    @State private var password = ""
    @State private var fullName = ""
    @State private var busy = false
    @State private var info: String?
    @State private var error: String?
    @State private var showServer = false

    var body: some View {
        VStack(spacing: 16) {
            Image(systemName: "waveform.and.mic").font(.system(size: 44))
            Text("WarpTalk").font(.largeTitle.bold())
            Picker("", selection: $registering) {
                Text("Đăng nhập").tag(false)
                Text("Đăng ký").tag(true)
            }.pickerStyle(.segmented).frame(width: 260)

            VStack(spacing: 10) {
                if registering { TextField("Họ tên", text: $fullName) }
                TextField("Email", text: $email).textContentType(.username)
                SecureField("Mật khẩu", text: $password).textContentType(.password)
            }
            .textFieldStyle(.roundedBorder).frame(width: 300)

            if let error { Text(error).foregroundStyle(.red).font(.callout).frame(width: 300) }
            if let info { Text(info).foregroundStyle(.green).font(.callout).frame(width: 300) }

            Button(action: submit) {
                if busy { ProgressView().controlSize(.small) } else { Text(registering ? "Tạo tài khoản" : "Đăng nhập") }
            }
            .keyboardShortcut(.defaultAction)
            .disabled(busy || email.isEmpty || password.isEmpty || (registering && fullName.isEmpty))

            DisclosureGroup("Server: \(settings.backendURL)", isExpanded: $showServer) {
                TextField("http://localhost:8080", text: $settings.backendURL).textFieldStyle(.roundedBorder)
            }.frame(width: 300).font(.caption)
        }
        .padding(40)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private func submit() {
        busy = true; error = nil; info = nil
        Task {
            defer { busy = false }
            do {
                if registering {
                    _ = try await app.api.register(email: email, password: password, fullName: fullName)
                    // Backend may require email verification (EMAIL_NOT_VERIFIED on login).
                    registering = false
                    info = "Đã tạo tài khoản. Nếu server yêu cầu xác thực email, hãy xác thực rồi đăng nhập."
                } else {
                    let s = try await app.api.login(email: email, password: password)
                    app.signedIn(s)
                }
            } catch {
                self.error = (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
            }
        }
    }
}
