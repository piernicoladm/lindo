import Foundation

enum ScanOutcome {
    case complete, partial, interrupted
    init(cancelled: Bool, partial: Bool) {
        self = cancelled ? .interrupted : partial ? .partial : .complete
    }
}

extension AppModel {
    func scanOutcome(for page: SidebarItem) -> ScanOutcome? {
        guard scanDates[page] != nil else { return nil }
        if let outcome = scanOutcomes[page] { return outcome }
        if page == .spaceMap, let report = spaceReport {
            return ScanOutcome(cancelled: report.cancelled, partial: report.isPartial)
        }
        return (warnings[page] ?? []).isEmpty ? .complete : .partial
    }

    func scanSummary(for page: SidebarItem) -> String {
        if scanning && activityPage == page { return scanCancelling ? "Interruzione…" : "Analisi in corso…" }
        guard let outcome = scanOutcome(for: page) else { return "" }
        let detail = page == .spaceMap ? "sola lettura" : page == .duplicates ? "copie extra" : elementCount(items[page]?.count ?? 0)
        switch outcome {
        case .interrupted: return "Interrotta · \(detail)"
        case .partial: return "Parziale · \(detail)"
        case .complete: return page == .spaceMap ? "Allocati · sola lettura" : page == .duplicates ? "Copie extra" : detail
        }
    }

    func emptyScanMessage(for page: SidebarItem) -> (title: String, detail: String) {
        switch scanOutcome(for: page) {
        case .interrupted:
            return ("Analisi interrotta", "Non ci sono risultati da mostrare. Ripeti l’analisi per completare la ricerca.")
        case .partial:
            return ("Analisi parziale, nessun risultato", "Alcuni elementi non sono stati esaminati. Apri i dettagli della copertura prima di riprovare.")
        default:
            return page == .duplicates
                ? ("Nessun duplicato trovato", "Non sono emerse copie identiche nei file esaminati. I file vuoti sono esclusi.")
                : ("Nessun candidato trovato", "L’analisi non ha trovato elementi nei percorsi accessibili con questi criteri.")
        }
    }
}
