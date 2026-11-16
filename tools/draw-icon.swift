import AppKit
import Foundation

// Original Open sheet artwork, matching Brand.swift and Icon/Mark.svg.
// SVG-style coordinates, with the origin at the upper-left of a 1024-unit canvas.
let destination = URL(fileURLWithPath: CommandLine.arguments[1], isDirectory: true)
func sheetPath() -> CGPath {
    let p = CGMutablePath()
    p.move(to: CGPoint(x: 326, y: 250))
    p.addLine(to: CGPoint(x: 602, y: 250))
    p.addLine(to: CGPoint(x: 774, y: 422))
    p.addLine(to: CGPoint(x: 774, y: 698))
    p.addQuadCurve(to: CGPoint(x: 698, y: 774), control: CGPoint(x: 774, y: 774))
    p.addLine(to: CGPoint(x: 326, y: 774))
    p.addQuadCurve(to: CGPoint(x: 250, y: 698), control: CGPoint(x: 250, y: 774))
    p.addLine(to: CGPoint(x: 250, y: 326))
    p.addQuadCurve(to: CGPoint(x: 326, y: 250), control: CGPoint(x: 250, y: 250))
    p.closeSubpath()
    p.move(to: CGPoint(x: 390, y: 410))
    p.addQuadCurve(to: CGPoint(x: 374, y: 426), control: CGPoint(x: 374, y: 410))
    p.addLine(to: CGPoint(x: 374, y: 634))
    p.addQuadCurve(to: CGPoint(x: 390, y: 650), control: CGPoint(x: 374, y: 650))
    p.addLine(to: CGPoint(x: 634, y: 650))
    p.addQuadCurve(to: CGPoint(x: 650, y: 634), control: CGPoint(x: 650, y: 650))
    p.addLine(to: CGPoint(x: 650, y: 426))
    p.addQuadCurve(to: CGPoint(x: 634, y: 410), control: CGPoint(x: 650, y: 410))
    p.closeSubpath()
    return p
}
func foldPath() -> CGPath {
    let p = CGMutablePath()
    p.move(to: CGPoint(x: 602, y: 250))
    p.addLine(to: CGPoint(x: 774, y: 422))
    p.addLine(to: CGPoint(x: 650, y: 422))
    p.addQuadCurve(to: CGPoint(x: 602, y: 374), control: CGPoint(x: 602, y: 422))
    p.closeSubpath()
    return p
}
for size in [16, 32, 128, 256, 512] {
    for scale in [1, 2] {
        let pixels = size * scale
        let bitmap = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: pixels,
            pixelsHigh: pixels, bitsPerSample: 8, samplesPerPixel: 4,
            hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB,
            bytesPerRow: 0, bitsPerPixel: 0)!
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: bitmap)
        let context = NSGraphicsContext.current!.cgContext
        context.scaleBy(x: CGFloat(pixels) / 1024, y: CGFloat(pixels) / 1024)
        context.translateBy(x: 0, y: 1024)
        context.scaleBy(x: 1, y: -1)
        context.setFillColor(NSColor(srgbRed: 72/255, green: 89/255, blue: 232/255, alpha: 1).cgColor)
        context.addPath(CGPath(roundedRect: CGRect(x: 62, y: 62, width: 900, height: 900),
                               cornerWidth: 205, cornerHeight: 205, transform: nil))
        context.fillPath()
        context.setFillColor(NSColor.white.cgColor)
        context.addPath(sheetPath())
        context.drawPath(using: .eoFill)
        context.setFillColor(NSColor(srgbRed: 182/255, green: 191/255, blue: 1, alpha: 1).cgColor)
        context.addPath(foldPath())
        context.fillPath()
        NSGraphicsContext.restoreGraphicsState()
        let suffix = scale == 2 ? "@2x" : ""
        let file = destination.appendingPathComponent("icon_\(size)x\(size)\(suffix).png")
        try bitmap.representation(using: .png, properties: [:])!.write(to: file)
    }
}
