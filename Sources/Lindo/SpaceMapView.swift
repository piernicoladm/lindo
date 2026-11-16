import AppKit
import LindoKit
import SwiftUI

struct SpaceMapView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 8) {
                header
                scopeCard
                if let report = model.spaceReport {
                    summary(report)
                    if report.isPartial || report.cancelled || !report.warnings.isEmpty {
                        partialNotice(report)
                    }
                    entries(report)
                } else if model.scanning && model.activePage == .spaceMap {
                    measuringState
                } else {
                    emptyState
                }
            }
            .padding(20)
            .frame(maxWidth: .infinity, alignment: .leading)
            .gridArea("space-map", inset: 20)
        }
        .scrollBounceBehavior(.basedOnSize)
    }

    private var header: some View {
        HStack(alignment: .top, spacing: 16) {
            Text("Mappa spazio").font(.system(size: 20, weight: .semibold))
            Spacer(minLength: 0)
            Label("Sola lettura", systemImage: "eye")
                .font(.system(size: 12)).foregroundStyle(.secondary)
        }
        .gridProbe("space-map-header", area: "space-map")
    }

    private var scopeCard: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(alignment: .center, spacing: 16) {
                Picker("Dove analizzare", selection: Binding(get: { model.spaceScope }, set: { model.setSpaceScope($0) })) {
                    ForEach(SpaceScope.allCases) { scope in Text(scope.title).tag(scope) }
                }
                .pickerStyle(.segmented).labelsHidden().frame(maxWidth: 480)
                .disabled(model.isBusy || model.isSample)
                .accessibilityIdentifier("space-map-scope")
                Spacer(minLength: 0)
                Button("Analizza", systemImage: "chart.bar.xaxis") { model.startSpaceScan() }
                    .buttonStyle(.bordered).controlSize(.regular)
                    .disabled(model.isBusy || model.isSample)
                    .accessibilityIdentifier("space-map-scan")
            }
            Text(model.spaceScope.detail).font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            Divider()
            HStack(spacing: 12) {
                Button {
                    model.startSpaceScan(at: model.spaceRoot.deletingLastPathComponent())
                } label: { Label("Cartella superiore", systemImage: "arrow.up") }
                .buttonStyle(.bordered)
                .disabled(!model.spaceCanGoBack || model.isBusy || model.isSample)
                .help("Analizza la cartella superiore")
                .accessibilityLabel("Torna alla cartella superiore e analizza")
                .accessibilityIdentifier("space-map-parent")
                Text(model.spaceRoot.path).font(.caption).foregroundStyle(.secondary).textSelection(.enabled)
                    .lineLimit(1).truncationMode(.middle)
                    .help(model.spaceRoot.path)
                Spacer(minLength: 0)
                Button("Mostra nel Finder", systemImage: "folder") { reveal(model.spaceRoot) }
                    .buttonStyle(.bordered).disabled(model.isSample)
                    .help(model.spaceRoot.path)
            }
        }
        .padding(.vertical, 4)
        .gridProbe("space-map-scope", area: "space-map")
    }

    private func summary(_ report: SpaceReport) -> some View {
        HStack(alignment: .top, spacing: 24) {
            metric("Spazio allocato", value: bytes(report.allocatedBytes))
                .help("Spazio assegnato sul disco ai file rilevati, secondo i metadati del filesystem.")
            Spacer(minLength: 0)
            metric("Dimensione logica", value: bytes(report.logicalBytes))
                .help("Dimensione del contenuto dei file, indipendente dallo spazio assegnato sul disco.")
            Spacer(minLength: 0)
            metric("File distinti", value: report.fileCount.formatted())
                .help("Gli hard link allo stesso file sono contati una sola volta nell’ambito analizzato.")
        }
        .padding(.vertical, 8)
        .overlay(alignment: .top) { Divider() }
        .overlay(alignment: .bottom) { Divider() }
        .gridProbe("space-map-summary", area: "space-map")
    }

    private func metric(_ title: String, value: String) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(title).font(.caption).foregroundStyle(.secondary)
            Text(value).font(.system(size: 17, weight: .semibold)).monospacedDigit().lineLimit(1).minimumScaleFactor(0.7)
        }
    }

    private func partialNotice(_ report: SpaceReport) -> some View {
        HStack(alignment: .top, spacing: 12) {
            Image(systemName: "exclamationmark.circle").foregroundStyle(.secondary)
            Text(report.cancelled ? "Analisi interrotta. Le dimensioni possono essere incomplete."
                 : report.coverage.hasUnplannedOmissions ? "Analisi parziale. Alcuni percorsi non sono verificabili; le dimensioni possono essere incomplete."
                 : "Analisi completata con esclusioni previste (\(report.coverage.excludedCount)). Le dimensioni riguardano gli elementi esaminati; i percorsi esclusi restano nei dettagli.")
                .font(.system(size: 12)).foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 0)
            if !report.warnings.isEmpty {
                Button("Dettagli") { model.showWarnings = true }.buttonStyle(.bordered)
            }
        }
        .padding(.vertical, 4)
    }

    private func entries(_ report: SpaceReport) -> some View {
        let sortedEntries = report.entries.sorted { $0.allocatedBytes > $1.allocatedBytes }
        return VStack(alignment: .leading, spacing: 8) {
            HStack {
                Text("Dentro questa cartella").font(.system(size: 13, weight: .semibold))
                Spacer()
                Text("Spazio allocato").font(.caption).foregroundStyle(.secondary)
            }
            if report.entries.isEmpty {
                Text("Nessun elemento misurabile in questa cartella.").font(.system(size: 13)).foregroundStyle(.secondary).padding(.vertical, 20)
            } else {
                LazyVStack(spacing: 0) {
                    ForEach(sortedEntries, id: \.id) { entry in
                        entryRow(entry, total: report.allocatedBytes)
                        if entry.id != sortedEntries.last?.id {
                            Divider()
                        }
                    }
                }
                .overlay(alignment: .top) { Divider() }
                .overlay(alignment: .bottom) { Divider() }
            }
            Text("Scegli una cartella nei risultati per misurarla più in dettaglio.")
                .font(.caption).foregroundStyle(.secondary)
            if report.entries.contains(where: { $0.alreadyCountedLinks > 0 }) {
                Text("Gli hard link sono nomi dello stesso file. Il peso è assegnato alla prima cartella incontrata; analizzando una sottocartella da sola l’attribuzione può cambiare.")
                    .font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            }
            Text("Allocato: spazio assegnato sul disco. Logico: dimensione del contenuto. Le barre confrontano lo spazio allocato; compressione e cloni possono influire sulle misure. Questi valori non indicano lo spazio recuperabile.")
                .font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
        }
        .gridProbe("space-map-entries", area: "space-map")
    }

    private func entryRow(_ entry: SpaceEntry, total: Int64) -> some View {
        HStack(spacing: 12) {
            Image(systemName: entry.isDirectory ? "folder.fill" : "doc.fill")
                .font(.system(size: 15)).foregroundStyle(.secondary).frame(width: 20)
            VStack(alignment: .leading, spacing: 4) {
                HStack(alignment: .firstTextBaseline) {
                    if entry.isDirectory {
                        Button { model.startSpaceScan(at: entry.url) } label: {
                            HStack(spacing: 8) {
                                Text(entry.url.lastPathComponent).lineLimit(1)
                                Image(systemName: "chevron.right").font(.caption2.weight(.semibold))
                            }
                        }
                        .buttonStyle(.plain).foregroundStyle(.primary)
                        .disabled(model.isBusy || model.isSample)
                        .help("Misura in dettaglio: \(entry.url.path)")
                        .accessibilityLabel("Analizza la cartella \(entry.url.lastPathComponent)")
                    } else {
                        Text(entry.url.lastPathComponent).lineLimit(1).help(entry.url.path)
                    }
                    if entry.coverage.hasUnplannedOmissions { Text("Parziale").font(.caption2).foregroundStyle(.secondary) }
                    else if entry.coverage.excludedCount > 0 { Text("Esclusioni").font(.caption2).foregroundStyle(.secondary) }
                    Spacer(minLength: 8)
                    Text(!entry.isDirectory && entry.alreadyCountedLinks > 0 ? "Già conteggiato" : bytes(entry.allocatedBytes)).monospacedDigit()
                        .accessibilityLabel(!entry.isDirectory && entry.alreadyCountedLinks > 0 ? "File già conteggiato in un’altra voce" : "Spazio allocato: \(bytes(entry.allocatedBytes))")
                }
                .font(.system(size: 13, weight: .medium))
                GeometryReader { geometry in
                    ZStack(alignment: .leading) {
                        Rectangle().fill(Palette.secondarySurface)
                        Rectangle().fill(Color.secondary.opacity(0.5))
                            .frame(width: geometry.size.width * min(1, max(0, total > 0 ? Double(entry.allocatedBytes) / Double(total) : 0)))
                    }
                }
                .frame(height: 4).accessibilityHidden(true)
                HStack {
                    if !entry.isDirectory && entry.alreadyCountedLinks > 0 {
                        Text("Hard link a un file già incluso nel totale")
                    } else {
                        Text("\(entry.fileCount.formatted()) file · \(bytes(entry.logicalBytes)) logici")
                    }
                    Spacer()
                    Button("Mostra nel Finder") { reveal(entry.url) }.buttonStyle(.plain).foregroundStyle(Palette.accentText)
                        .disabled(model.isSample).help(entry.url.path)
                        .accessibilityLabel("Mostra nel Finder")
                        .accessibilityValue(entry.url.path)
                }
                .font(.caption).foregroundStyle(.secondary)
                if entry.isDirectory && entry.alreadyCountedLinks > 0 {
                    Text("\(entry.alreadyCountedLinks) hard link già conteggiati in altre voci")
                        .font(.caption).foregroundStyle(.secondary)
                }
            }
        }
        .padding(.vertical, 8)
    }

    private var measuringState: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("Misurazione di \(model.spaceRoot.lastPathComponent)…")
                .font(.headline)
            Text("File e cartelle appariranno qui al termine. Puoi seguire o interrompere l'analisi nella barra attività.")
                .font(.system(size: 13)).foregroundStyle(.secondary)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.vertical, 16)
        .accessibilityIdentifier("space-map-measuring")
    }

    private var emptyState: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("Nessuna misurazione").font(.system(size: 17, weight: .semibold))
            Text("Scegli un ambito sopra e premi Analizza. Vedrai file e cartelle ordinati per spazio allocato.")
                .font(.system(size: 13)).foregroundStyle(.secondary)
        }
        .frame(maxWidth: .infinity, alignment: .leading).padding(.vertical, 16)
        .gridProbe("space-map-empty", area: "space-map")
    }

    private func reveal(_ url: URL) { NSWorkspace.shared.activateFileViewerSelecting([url]) }
}
