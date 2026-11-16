import AppKit
import LindoKit
import SwiftUI

struct ConfirmationView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            VStack(alignment: .leading, spacing: 8) {
                Text("Rivedi la selezione").font(.system(size: 20, weight: .semibold))
                Text("Controlla i percorsi prima di spostare gli elementi nel Cestino.")
                    .font(.system(size: 13)).foregroundStyle(.secondary)
            }

            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    VStack(alignment: .leading, spacing: 0) {
                        reviewHeading("Nel Cestino", symbol: "trash", count: model.reviewItems.count, size: model.reviewBytes)
                        Divider()
                        LazyVStack(alignment: .leading, spacing: 0) {
                            ForEach(model.reviewItems) { item in
                                reviewRow(item)
                                if item.id != model.reviewItems.last?.id { Divider() }
                            }
                        }
                    }

                    if model.reviewPage == .duplicates {
                        VStack(alignment: .leading, spacing: 0) {
                            reviewHeading("Restano sul Mac", symbol: "lock.shield", count: model.preservedReviewFiles.count,
                                size: model.preservedReviewFiles.reduce(0) { $0 + $1.bytes })
                            Divider()
                            LazyVStack(alignment: .leading, spacing: 0) {
                                ForEach(model.preservedReviewFiles) { item in
                                    reviewRow(item)
                                    if item.id != model.preservedReviewFiles.last?.id { Divider() }
                                }
                            }
                        }
                    }
                }
            }
            .frame(minHeight: 160, maxHeight: 320)
            .scrollBounceBehavior(.basedOnSize)

            VStack(alignment: .leading, spacing: 8) {
                Label("Puoi ripristinare i file dal Finder finché non svuoti il Cestino.", systemImage: "arrow.uturn.backward.circle")
                    .font(.system(size: 13, weight: .medium))
                if model.reviewSyncStatus == .synced {
                    Label("L’analisi ha rilevato elementi sincronizzati. Spostarli nel Cestino li rimuove anche dagli altri dispositivi e dalle cartelle condivise.", systemImage: "exclamationmark.icloud")
                        .font(.system(size: 12, weight: .medium)).foregroundStyle(.orange)
                        .fixedSize(horizontal: false, vertical: true)
                        .accessibilityIdentifier("synced-folder-warning")
                } else if model.reviewSyncStatus == .unknown {
                    Label("Non è stato possibile verificare la sincronizzazione di alcuni elementi. Se sono sincronizzati, il Cestino li rimuoverà anche dagli altri dispositivi e dalle cartelle condivise. Controlla i percorsi prima di continuare.", systemImage: "exclamationmark.triangle")
                        .font(.system(size: 12, weight: .medium)).foregroundStyle(.orange)
                        .fixedSize(horizontal: false, vertical: true)
                        .accessibilityIdentifier("unknown-sync-warning")
                }
                Text(model.reviewPage == .duplicates
                    ? "La copia protetta viene ricontrollata prima di ogni rimozione. Se manca o se una copia cambia, la rimozione viene bloccata. Le dimensioni sono logiche: lo spazio recuperato può variare."
                    : model.reviewPage == .applications
                        ? "Viene spostato solo il bundle .app. I dati personali restano al loro posto. Le app in esecuzione e gli elementi modificati dopo l’analisi vengono esclusi."
                        : model.reviewPage == .cleanup
                            ? "I browser aperti bloccano la pulizia della loro cache. I file modificati dopo l’analisi vengono esclusi. Le cache riconosciute possono essere ricreate dai browser."
                            : "Vengono spostati solo i file selezionati. I file modificati dopo l’analisi vengono esclusi. Le dimensioni sono logiche: lo spazio recuperato può variare.")
                    .font(.system(size: 12)).foregroundStyle(.secondary)
            }
            Divider()
            HStack(spacing: 12) {
                Text("\(elementCount(model.reviewItems.count)) · \(bytes(model.reviewBytes))")
                    .font(.system(size: 13, weight: .semibold)).monospacedDigit()
                Spacer(minLength: 0)
                Button("Torna alla selezione", role: .cancel) { dismiss() }
                    .keyboardShortcut(.cancelAction).controlSize(.regular)
                Button("Sposta nel Cestino", role: .destructive) {
                    dismiss()
                    model.trashSelection()
                }
                .buttonStyle(.borderedProminent).tint(.red).controlSize(.regular)
                .disabled(model.isBusy || model.isSample || model.reviewItems.isEmpty)
                .accessibilityIdentifier("confirm-trash")
            }
        }
        .padding(20).frame(width: 640).background(Palette.canvas).noInitialFocus()
    }

    private func reviewHeading(_ title: String, symbol: String, count: Int, size: Int64) -> some View {
        HStack(spacing: 12) {
            Label(title, systemImage: symbol).font(.system(size: 13, weight: .semibold))
            Spacer(minLength: 0)
            Text("\(elementCount(count)) · \(bytes(size))")
                .font(.system(size: 12)).foregroundStyle(.secondary).monospacedDigit()
        }.padding(.vertical, 8)
    }

    private func reviewRow(_ item: ScanItem) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 12) {
                Text(item.title).font(.system(size: 13, weight: .medium))
                Spacer(minLength: 0)
                Text(bytes(item.bytes)).font(.system(size: 12)).foregroundStyle(.secondary).monospacedDigit()
                Button { NSWorkspace.shared.activateFileViewerSelecting([item.url]) } label: {
                    Image(systemName: "folder")
                }.buttonStyle(.borderless).disabled(model.isSample)
                    .help("Mostra nel Finder").accessibilityLabel("Mostra \(item.title) nel Finder")
            }
            Text(item.url.path).font(.system(size: 12)).foregroundStyle(.secondary)
                .textSelection(.enabled).fixedSize(horizontal: false, vertical: true)
        }.padding(.vertical, 8).frame(maxWidth: .infinity, alignment: .leading)
    }
}

struct DetailsSheet: View {
    let title: String
    let lines: [String]
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text(title).font(.system(size: 20, weight: .semibold))
            Text("\(lines.count) \(lines.count == 1 ? "dettaglio" : "dettagli")")
                .font(.system(size: 13)).foregroundStyle(.secondary)
            ScrollView {
                VStack(alignment: .leading, spacing: 12) {
                    ForEach(Array(lines.enumerated()), id: \.offset) { _, line in
                        Text(line).font(.system(size: 13)).textSelection(.enabled)
                            .frame(maxWidth: .infinity, alignment: .leading)
                    }
                }.padding(.vertical, 8)
            }.frame(height: 280)
            .overlay(alignment: .top) { Divider() }
            .overlay(alignment: .bottom) { Divider() }
            HStack { Spacer(); Button("Chiudi") { dismiss() }.keyboardShortcut(.cancelAction).controlSize(.regular) }
        }.padding(20).frame(width: 580).background(Palette.canvas).noInitialFocus()
    }
}
