import AppKit
import LindoKit
import SwiftUI

struct ContentView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.openSettings) private var openSettings

    var body: some View {
        @Bindable var model = model
        NavigationSplitView {
            VStack(spacing: 0) {
                List(selection: $model.sidebar) {
                    Section("Spazio") {
                        navigationRow(.overview)
                        navigationRow(.spaceMap)
                    }
                    Section("Pulizia") {
                        navigationRow(.cleanup)
                        navigationRow(.largeFiles)
                        navigationRow(.duplicates)
                        navigationRow(.applications)
                    }
                }
                .listStyle(.sidebar)
                .disabled(model.showConfirmation || model.choosingFolder)
                Divider()
                HStack {
                    Button("Impostazioni…", systemImage: "gearshape") { openSettings() }
                        .buttonStyle(.plain).foregroundStyle(.secondary)
                        .font(.caption).help("Impostazioni (⌘,)")
                    Spacer(minLength: 0)
                }.padding(12)
            }
            .navigationSplitViewColumnWidth(min: 180, ideal: 196, max: 240)
        } detail: {
            Group {
                if model.sidebar == .overview { OverviewView() }
                else if model.sidebar == .spaceMap { SpaceMapView() }
                else if model.sidebar == .duplicates { DuplicateView() }
                else { ResultsView() }
            }
            .background(Palette.canvas)
            .safeAreaInset(edge: .bottom, spacing: 0) {
                if (model.scanning || model.cleaning), let page = model.activityPage {
                    HStack(spacing: 12) {
                        ProgressView().controlSize(.small)
                        VStack(alignment: .leading, spacing: 4) {
                            Text(model.cleaning ? "Verifica e spostamento nel Cestino…" : "Analisi: \(page.title)")
                                .font(.callout.weight(.semibold))
                            Text(model.cleaning
                                 ? "\(page.title) · La verifica dei file può richiedere tempo."
                                 : model.progressText)
                                .font(.caption).foregroundStyle(.secondary)
                                .lineLimit(2)
                        }
                        Spacer(minLength: 0)
                        if model.sidebar != page {
                            Button("Mostra operazione") { model.sidebar = page }
                                .accessibilityIdentifier("activity-return")
                        }
                        if model.scanning && !model.cleaning {
                            Button(model.scanCancelling ? "Interruzione…" : "Interrompi") { model.cancelScan() }
                                .disabled(model.scanCancelling)
                                .accessibilityIdentifier("global-scan-stop")
                        }
                    }
                    .padding(12).frame(maxWidth: .infinity, alignment: .leading)
                    .background(Palette.canvas)
                    .overlay(alignment: .top) { Divider() }
                    .accessibilityElement(children: .contain)
                    .accessibilityIdentifier(model.cleaning ? "trash-progress" : "scan-progress")
                }
            }
            .toolbar {
                if model.isSample {
                    ToolbarItem(placement: .automatic) {
                        Label("Dati di esempio", systemImage: "eye")
                            .font(.caption).foregroundStyle(.secondary)
                    }
                }
            }
        }
        .onChange(of: model.sidebar) { _, _ in model.pageChanged() }
        .sheet(isPresented: $model.showConfirmation) { ConfirmationView() }
        .sheet(isPresented: $model.showWarnings) {
            DetailsSheet(title: "Dettagli della scansione", lines: model.warnings[model.sidebar] ?? [])
        }
    }

    private func navigationRow(_ page: SidebarItem) -> some View {
        Label(page.title, systemImage: page.symbol)
            .font(.body)
            .tag(page)
            .accessibilityIdentifier("nav-\(page.rawValue)")
    }
}

struct OverviewView: View {
    @Environment(AppModel.self) private var model
    private let tools: [SidebarItem] = [.spaceMap, .cleanup, .largeFiles, .duplicates, .applications]

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 24) {
                driveSummary.gridProbe("overview-disk", area: "overview")
                VStack(alignment: .leading, spacing: 0) {
                    HStack {
                        Text("Analisi").font(.headline)
                        Spacer()
                        Text("Dimensioni rilevate").font(.caption).foregroundStyle(.secondary)
                            .help("Le dimensioni dipendono dall'ambito analizzato e non indicano spazio immediatamente recuperabile.")
                    }.padding(.bottom, 12)
                    Divider()
                    ForEach(tools) { page in
                        toolRow(page)
                        Divider()
                    }
                }
                .gridProbe("overview-tools", area: "overview")
                Text("Scegli un'analisi per esaminare i file. La mappa misura lo spazio senza proporre rimozioni.")
                    .font(.caption).foregroundStyle(.secondary)
                    .gridProbe("overview-footnote", area: "overview")
            }
            .padding(20)
            .frame(maxWidth: .infinity, alignment: .leading)
            .gridArea("overview", inset: 20)
        }
        .scrollBounceBehavior(.basedOnSize)
    }

    private var driveSummary: some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack(spacing: 16) {
                Image(systemName: "internaldrive")
                    .font(.system(size: 40, weight: .light)).foregroundStyle(.secondary)
                    .frame(width: 52, height: 52)
                VStack(alignment: .leading, spacing: 4) {
                    Text("Disco di avvio").font(.system(size: 20, weight: .semibold))
                    Text(model.disk.total > 0 ? "\(bytes(model.disk.total)) di capacità" : "Capacità non disponibile")
                        .font(.callout).foregroundStyle(.secondary)
                }
                Spacer(minLength: 0)
                VStack(alignment: .trailing, spacing: 4) {
                    Text(model.disk.total > 0 ? bytes(model.disk.available) : "Non disponibile")
                        .font(.system(size: 20, weight: .semibold)).monospacedDigit()
                    Text("disponibili").font(.caption).foregroundStyle(.secondary)
                }
            }
            if model.disk.total > 0 {
                ProgressView(value: model.disk.fraction)
                    .tint(.secondary)
                    .accessibilityLabel("Spazio occupato")
                    .accessibilityValue("\(Int(model.disk.fraction * 100)) per cento")
                HStack {
                    Text("\(bytes(model.disk.used)) occupati")
                    Spacer()
                    Text("\(Int(model.disk.fraction * 100))% del disco")
                }.font(.caption).foregroundStyle(.secondary)
            }
        }
    }

    private func toolRow(_ page: SidebarItem) -> some View {
        Button { model.sidebar = page } label: {
            HStack(spacing: 12) {
                Image(systemName: page.symbol).font(.system(size: 16))
                    .foregroundStyle(.secondary).frame(width: 24)
                VStack(alignment: .leading, spacing: 4) {
                    Text(page.title).font(.body.weight(.medium))
                    Text(description(page)).font(.caption).foregroundStyle(.secondary)
                }
                Spacer(minLength: 16)
                VStack(alignment: .trailing, spacing: 4) {
                    Text(measurement(page)).font(.body).monospacedDigit()
                    Text(model.scanSummary(for: page)).font(.caption)
                        .foregroundStyle(model.scanOutcome(for: page) == .complete ? Color.secondary : Color.primary)
                    LastScanLabel(date: model.scanDates[page], identifier: "last-scan-\(page.rawValue)")
                }
                Image(systemName: "chevron.right").font(.caption2).foregroundStyle(.tertiary)
            }
            .padding(.vertical, 12)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityIdentifier("open-\(page.rawValue)")
        .help("Apri \(page.title)")
    }

    private func description(_ page: SidebarItem) -> String {
        switch page {
        case .spaceMap: "Distribuzione dello spazio per cartella"
        case .cleanup: "Cache web e log meno recenti"
        case .largeFiles: "File oltre la dimensione scelta"
        case .duplicates: "Copie con contenuto identico"
        case .applications: "Applicazioni installate"
        case .overview: ""
        }
    }

    private func measurement(_ page: SidebarItem) -> String {
        guard model.scanDates[page] != nil else { return "Da analizzare" }
        if page == .spaceMap {
            return model.spaceReport.map { bytes($0.allocatedBytes) } ?? "In corso…"
        }
        if page == .duplicates { return bytes(model.duplicateExtraBytes) }
        return bytes((model.items[page] ?? []).reduce(0) { $0 + $1.bytes })
    }

}
