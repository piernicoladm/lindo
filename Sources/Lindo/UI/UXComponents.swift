import SwiftUI

struct SelectionFooter: View {
    let count: Int
    let size: Int64
    let note: String
    let disabled: Bool
    let identifier: String
    var hiddenCount: Int = 0
    var clearFilter: (() -> Void)? = nil
    let review: () -> Void

    var body: some View {
        HStack(spacing: 16) {
            VStack(alignment: .leading, spacing: 4) {
                Text(count == 0 ? "Seleziona gli elementi da rivedere" : "\(count == 1 ? "1 elemento selezionato" : "\(count) elementi selezionati") · \(bytes(size))")
                    .font(.system(size: 13, weight: .semibold))
                if hiddenCount > 0 {
                    Text(hiddenCount == 1 ? "1 elemento selezionato nascosto dal filtro: incluso nella revisione." : "\(hiddenCount) elementi selezionati nascosti dal filtro: inclusi nella revisione.")
                        .font(.system(size: 12, weight: .medium))
                }
                Text(note).font(.system(size: 11)).foregroundStyle(.secondary)
                    .lineLimit(2)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .layoutPriority(1)
            .drawingGroup()
            if hiddenCount > 0, let clearFilter {
                Button("Rimuovi filtro", action: clearFilter)
                    .disabled(disabled)
                    .accessibilityIdentifier("\(identifier)-clear-filter")
                    .help("Mostra tutti i risultati conservando le selezioni.")
                    .fixedSize(horizontal: true, vertical: false)
            }
            Button("Rivedi selezione…") { review() }
                .buttonStyle(.bordered)
                .disabled(disabled || count == 0)
                .accessibilityIdentifier(identifier)
                .fixedSize(horizontal: true, vertical: false)
        }
        .padding(.top, 12).frame(maxWidth: .infinity, alignment: .leading)
        .overlay(alignment: .top) { Divider() }
    }
}

/// Shows when the displayed scan ended, without implying that its files are unchanged.
struct LastScanLabel: View {
    let date: Date?
    let identifier: String

    var body: some View {
        if let date {
            Text("Ultima analisi: \(date.formatted(date: .abbreviated, time: .shortened))")
                .font(.caption).foregroundStyle(.secondary)
                .help("Data di conclusione dell’analisi mostrata. I file possono essere cambiati da allora.")
                .accessibilityIdentifier(identifier)
        }
    }
}

struct ResultsSearchField: View {
    @Binding var query: String
    let prompt: String
    let identifier: String
    var focused: FocusState<Bool>.Binding
    let clear: () -> Void

    var body: some View {
        HStack(spacing: 4) {
            TextField(prompt, text: $query)
                .textFieldStyle(.roundedBorder)
                .accessibilityLabel(prompt)
                .accessibilityIdentifier(identifier)
                .focused(focused)
                .onExitCommand { clear() }
            if !query.isEmpty {
                Button {
                    clear()
                    focused.wrappedValue = true
                } label: {
                    Image(systemName: "xmark.circle.fill")
                        .frame(width: 24, height: 24)
                }
                .buttonStyle(.borderless)
                .accessibilityLabel("Cancella ricerca")
                .accessibilityIdentifier("\(identifier)-clear")
                .help("Cancella ricerca")
            }
        }.frame(width: 200)
    }
}
