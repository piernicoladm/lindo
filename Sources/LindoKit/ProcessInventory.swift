import Foundation
import Darwin

public struct ProcessSnapshot: Sendable {
    public let paths: Set<String>
    public let isComplete: Bool
    public init(paths: Set<String>, isComplete: Bool) { self.paths = paths; self.isComplete = isComplete }
}

public enum ProcessInventory {
    /// Native process enumeration uses effective UID and executable paths only.
    /// Zombies and processes that exit during inspection cannot hold browser caches open.
    public static func capture() -> ProcessSnapshot {
        let required = proc_listpids(UInt32(PROC_UID_ONLY), getuid(), nil, 0)
        guard required > 0 else { return ProcessSnapshot(paths: [], isComplete: false) }
        var pids = [pid_t](repeating: 0, count: Int(required) / MemoryLayout<pid_t>.size + 512)
        let capacity = pids.count * MemoryLayout<pid_t>.size
        let bytes = pids.withUnsafeMutableBytes { proc_listpids(UInt32(PROC_UID_ONLY), getuid(), $0.baseAddress, Int32(capacity)) }
        guard bytes > 0, Int(bytes) < capacity else { return ProcessSnapshot(paths: [], isComplete: false) }
        var paths = Set<String>(), complete = true
        for pid in pids.prefix(Int(bytes) / MemoryLayout<pid_t>.size) where pid > 0 {
            var info = proc_bsdinfo()
            let infoSize = Int32(MemoryLayout<proc_bsdinfo>.size)
            errno = 0
            let read = proc_pidinfo(pid, PROC_PIDTBSDINFO, 0, &info, infoSize)
            if read != infoSize {
                if errno != ESRCH { complete = false }
                continue
            }
            guard info.pbi_uid == getuid() else { continue }
            if info.pbi_status == UInt32(SZOMB) { continue }
            var buffer = [CChar](repeating: 0, count: 4 * Int(MAXPATHLEN))
            let length = buffer.withUnsafeMutableBytes { proc_pidpath(pid, $0.baseAddress, UInt32($0.count)) }
            guard length > 0 else {
                let pathError = errno
                let executableName = withUnsafeBytes(of: info.pbi_name) { String(decoding: $0.prefix { $0 != 0 }, as: UTF8.self) }
                if let mappedPath = mappedExecutablePath(pid: pid, executableName: executableName) {
                    paths.insert(mappedPath)
                    continue
                }
                var current = proc_bsdinfo()
                errno = 0
                let stillExists = proc_pidinfo(pid, PROC_PIDTBSDINFO, 0, &current, infoSize)
                if pathError != ESRCH && !(stillExists == infoSize && current.pbi_status == UInt32(SZOMB)) && !(stillExists <= 0 && errno == ESRCH) { complete = false }
                continue
            }
            let path = String(decoding: buffer.prefix { $0 != 0 }.map { UInt8(bitPattern: $0) }, as: UTF8.self)
            if path.hasPrefix("/") { paths.insert(path) } else { complete = false }
        }
        return ProcessSnapshot(paths: paths, isComplete: complete && !paths.isEmpty)
    }
    // A running extension can retain its executable mapping after an update removes
    // its original vnode; proc_pidpath then returns ENOENT. Inspect only file paths
    // of executable memory mappings, never their bytes, process arguments or environment.
    private static func mappedExecutablePath(pid: pid_t, executableName: String) -> String? {
        guard !executableName.isEmpty else { return nil }
        var address: UInt64 = 0, matches = Set<String>()
        for _ in 0..<4096 {
            var region = proc_regionwithpathinfo()
            let size = Int32(MemoryLayout<proc_regionwithpathinfo>.size)
            guard proc_pidinfo(pid, PROC_PIDREGIONPATHINFO, address, &region, size) == size else { break }
            if region.prp_prinfo.pri_protection & UInt32(VM_PROT_EXECUTE) != 0 {
                let path = withUnsafeBytes(of: region.prp_vip.vip_path) { String(decoding: $0.prefix { $0 != 0 }, as: UTF8.self) }
                if matchesExecutable(path: path, name: executableName) { matches.insert(path) }
            }
            let (next, overflow) = region.prp_prinfo.pri_address.addingReportingOverflow(region.prp_prinfo.pri_size)
            if overflow || next <= address { return nil }
            address = next
        }
        return matches.count == 1 ? matches.first : nil
    }

    static func matchesExecutable(path: String, name: String) -> Bool {
        // pbi_name is limited to 32 bytes. A longer executable name can be truncated.
        path.hasPrefix("/") && !name.isEmpty && URL(fileURLWithPath: path).lastPathComponent.hasPrefix(name)
    }
}
