/// Separates known policy exclusions from failures to inspect the requested scope.
/// Excluded counts refer to skipped entries, not an estimate of their descendants.
public struct ScanCoverage: Sendable, Equatable {
    public private(set) var excludedCount: Int
    public private(set) var hasUnplannedOmissions: Bool

    public init(excludedCount: Int = 0, hasUnplannedOmissions: Bool = false) {
        self.excludedCount = max(0, excludedCount)
        self.hasUnplannedOmissions = hasUnplannedOmissions
    }

    mutating func exclude(_ count: Int = 1) { excludedCount += max(0, count) }
    mutating func fail() { hasUnplannedOmissions = true }
    mutating func merge(_ other: ScanCoverage) {
        excludedCount += other.excludedCount
        hasUnplannedOmissions = hasUnplannedOmissions || other.hasUnplannedOmissions
    }

    public static func combined<S: Sequence>(_ values: S) -> ScanCoverage where S.Element == ScanCoverage {
        var result = ScanCoverage()
        for value in values {
            result.merge(value)
        }
        return result
    }
}
