import SwiftUI

/// Original folded-sheet mark. Geometry is shared with Icon/Mark.svg.
struct LindoMark: View {
    var size: CGFloat = 40

    init(size: CGFloat = 40) {
        self.size = size
    }

    var body: some View {
        ZStack {
            RoundedRectangle(cornerRadius: size * 0.20, style: .continuous)
                .fill(Palette.accent)
                .padding(size * 0.06)
            LindoGlyph(color: .white, foldColor: Color(red: 0.714, green: 0.749, blue: 1))
        }
        .frame(width: size, height: size)
        .accessibilityHidden(true)
    }
}

struct LindoGlyph: View {
    var color: Color = Palette.accent
    var foldColor: Color = Color(red: 0.659, green: 0.698, blue: 1)

    var body: some View {
        ZStack {
            OpenSheetShape().fill(color, style: FillStyle(eoFill: true))
            LiftedCornerShape().fill(foldColor)
        }
        .aspectRatio(1, contentMode: .fit)
    }
}

private struct OpenSheetShape: Shape {
    func path(in rect: CGRect) -> Path {
        var p = Path()
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
        return p.applying(CGAffineTransform(scaleX: rect.width / 1024, y: rect.height / 1024)
            .concatenating(CGAffineTransform(translationX: rect.minX, y: rect.minY)))
    }
}

private struct LiftedCornerShape: Shape {
    func path(in rect: CGRect) -> Path {
        var p = Path()
        p.move(to: CGPoint(x: 602, y: 250))
        p.addLine(to: CGPoint(x: 774, y: 422))
        p.addLine(to: CGPoint(x: 650, y: 422))
        p.addQuadCurve(to: CGPoint(x: 602, y: 374), control: CGPoint(x: 602, y: 422))
        p.closeSubpath()
        return p.applying(CGAffineTransform(scaleX: rect.width / 1024, y: rect.height / 1024)
            .concatenating(CGAffineTransform(translationX: rect.minX, y: rect.minY)))
    }
}
