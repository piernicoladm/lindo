import Foundation

enum ScanOutcome {
    case complete, excluded, partial, interrupted
    init(cancelled: Bool, partial: Bool, excludedCount: Int = 0) {
        self = cancelled ? .interrupted : partial ? .partial : excludedCount > 0 ? .excluded : .complete
    }
}

extension AppModel {
    func scanOutcome(for page: SidebarItem) -> ScanOutcome? {
        guard scanDates[page] != nil else { return nil }
        if let outcome = scanOutcomes[page] { return outcome }
        if page == .spaceMap, let report = spaceReport {
            return ScanOutcome(cancelled: report.cancelled, partial: report.coverage.hasUnplannedOmissions, excludedCount: report.coverage.excludedCount)
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
        case .excluded: return "Completata con esclusioni · \(detail)"
        }
    }

    func emptyScanMessage(for page: SidebarItem) -> (title: String, detail: String) {
        switch scanOutcome(for: page) {
        case .interrupted:
            return ("Analisi interrotta", "Non ci sono risultati da mostrare. Ripeti l’analisi per completare la ricerca.")
        case .partial:
            return ("Analisi parziale, nessun risultato", "Alcuni elementi non sono stati esaminati. Apri i dettagli della copertura prima di riprovare.")
        case .excluded:
            return ("Analisi completata con esclusioni", "Nessun candidato fra i file esaminati. Gli elementi esclusi dalle regole non sono stati analizzati; puoi consultarli nei dettagli.")
        default:
            return page == .duplicates
                ? ("Nessun duplicato trovato", "Non sono emerse copie identiche nei file esaminati. I file vuoti sono esclusi.")
                : ("Nessun candidato trovato", "L’analisi non ha trovato elementi nei percorsi accessibili con questi criteri.")
        }
    }

    func coverageNotice(for page: SidebarItem) -> String {
        scanOutcome(for: page) == .excluded ? "Analisi completata con esclusioni previste" : "Copertura parziale o elementi non verificabili"
    }
}
