import AppKit
import SwiftUI

@main
struct LindoApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var delegate
    @State private var model = AppModel()

    var body: some Scene {
        Window("Lindo", id: "main") {
            GeometryReader { _ in
                ContentView()
                    .environment(model)
                    .modifier(FolderPickerOverlay(model: model))
                    .tint(Palette.accent)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
                .frame(minWidth: 980, minHeight: 680)
                .noInitialFocus()
                .task {
                    delegate.model = model
                    Snapshot.runIfRequested(model: model)
                }
        }
        .defaultSize(width: 1140, height: 760)
        .windowToolbarStyle(.unified)
        .commands {
            SidebarCommands()
            CommandGroup(replacing: .newItem) {
                Button("Avvia analisi") { model.startScan() }
                    .keyboardShortcut("r")
                    .disabled(model.isBusy || model.isSample || model.showConfirmation)
                Button("Interrompi analisi") { model.cancelScan() }
                    .keyboardShortcut(".")
                    .disabled(!model.scanning)
                Divider()
                Button("Scegli cartella…") { if model.sidebar != .duplicates { model.sidebar = .largeFiles }; model.chooseFolder() }
                    .keyboardShortcut("o")
                    .disabled(model.isBusy || model.isSample || model.showConfirmation || model.sidebar == .spaceMap)
                Button("Rivedi selezione…") { model.reviewSelection() }
                    .keyboardShortcut("d", modifiers: [.command, .shift])
                    .disabled(model.selectedItems.isEmpty || model.isBusy)
            }
            CommandGroup(after: .textEditing) {
                Button("Cerca nei risultati") { model.searchFocusRequest += 1 }
                    .keyboardShortcut("f")
                    .disabled(model.currentItems.isEmpty || model.showConfirmation)
            }
            CommandMenu("Vai") {
                ForEach(Array(SidebarItem.allCases.enumerated()), id: \.element.id) { index, page in
                    Button(page.title) { model.sidebar = page }
                        .keyboardShortcut(KeyEquivalent(Character(String(index + 1))))
                        .disabled(model.showConfirmation || model.choosingFolder)
                }
            }
        }
        Settings {
            SettingsView()
                .tint(Palette.accent)
                .noInitialFocus()
        }
    }
}

final class AppDelegate: NSObject, NSApplicationDelegate {
    weak var model: AppModel?

    // Quitting or closing the window during a confirmed removal must not stop the
    // batch halfway without a result: let it finish, then quit.
    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        guard let model, model.cleaning else { return .terminateNow }
        model.terminateAfterCleaning = true
        return .terminateLater
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        NSApp.setActivationPolicy(.regular)
        NSApp.activate()
    }
    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { true }
}

struct SettingsView: View {
    var body: some View {
        Form {
            Section {
                HStack(spacing: 12) {
                    LindoMark(size: 32)
                    VStack(alignment: .leading, spacing: 4) {
                        Text("Lindo").font(.title2.weight(.semibold))
                        Text("Versione \(AppInfo.version)").font(.caption).foregroundStyle(.secondary)
                    }
                }.padding(.vertical, 8)
                LabeledContent("Elaborazione", value: "Sul tuo Mac")
                LabeledContent("Dati di scansione", value: "Solo in memoria")
                LabeledContent("Rimozione", value: "Tramite Cestino")
            }
            Section("Come funziona") {
                Text("La pulizia propone solo cache web riconosciute di Chrome, Chromium e Firefox, come cartelle intere. Prima di rimuoverle, chiudi il browser. I log sono proposti dopo 7 giorni dall’ultima modifica.")
                Text("I duplicati vengono verificati per contenuto. Ogni gruppo mantiene una copia da conservare, protetta dalla rimozione.")
                Text("La mappa misura solo metadati e può includere Library e cartelle nascoste. Esclude aree private e credenziali e non offre comandi di rimozione.")
                Text("Nella pulizia, cartelle protette, link simbolici, documenti in bundle e file cloud non scaricati vengono esclusi. I permessi di macOS possono limitare la scansione.")
                Text("La rimozione di un’app riguarda il bundle .app. Preferenze, documenti e altri dati dell’app restano al loro posto.")
            }
            Section("Scorciatoie") {
                LabeledContent("Cerca nei risultati", value: "⌘F")
                LabeledContent("Avvia / interrompi analisi", value: "⌘R / ⌘.")
                LabeledContent("Scegli cartella", value: "⌘O")
                LabeledContent("Rivedi selezione", value: "⇧⌘D")
                LabeledContent("Apri una sezione", value: "⌘1 ... ⌘6")
            }
        }
        .formStyle(.grouped)
        .frame(width: 560, height: 560)
        .gridArea("settings", inset: 20)
    }
}
