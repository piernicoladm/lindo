import Foundation
import Testing
@testable import LindoKit

@Test func nativeMappedExecutableIdentificationRequiresAbsoluteMatchingPath() {
    #expect(ProcessInventory.matchesExecutable(path: "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome", name: "Google Chrome"))
    #expect(ProcessInventory.matchesExecutable(path: "/Applications/Example.app/Contents/MacOS/Long executable name suffix", name: "Long executable name"))
    #expect(!ProcessInventory.matchesExecutable(path: "relative/Google Chrome", name: "Google Chrome"))
    #expect(!ProcessInventory.matchesExecutable(path: "/usr/lib/library.dylib", name: "Google Chrome"))
    #expect(!ProcessInventory.matchesExecutable(path: "/usr/bin/test", name: ""))
}
