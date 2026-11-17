// Reproducible Candid app icon, drawn from code; no external image assets.
// usage: create_icon.swift OUTPUT.png [ring|thread|orbit]   (default: ring)
import AppKit

let path = CommandLine.arguments[1]
let variant = CommandLine.arguments.count > 2 ? CommandLine.arguments[2] : "ring"
let size = 1024.0
let bitmap = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: Int(size), pixelsHigh: Int(size), bitsPerSample: 8, samplesPerPixel: 4,
                              hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
NSGraphicsContext.saveGraphicsState()
let graphics = NSGraphicsContext(bitmapImageRep: bitmap)!
NSGraphicsContext.current = graphics
let ctx = graphics.cgContext
let space = CGColorSpaceCreateDeviceRGB()
func color(_ r: Double, _ g: Double, _ b: Double, _ a: Double = 1) -> CGColor { CGColor(red: r, green: g, blue: b, alpha: a) }
func gradient(_ colors: [CGColor], _ locations: [CGFloat]) -> CGGradient { CGGradient(colorsSpace: space, colors: colors as CFArray, locations: locations)! }

// macOS icon grid: 824pt squircle centered in 1024.
let tile = CGRect(x: 100, y: 100, width: 824, height: 824)
let squircle = CGPath(roundedRect: tile, cornerWidth: 185, cornerHeight: 185, transform: nil)
ctx.clear(CGRect(x: 0, y: 0, width: size, height: size))

// Soft drop shadow under the tile.
ctx.saveGState()
ctx.setShadow(offset: CGSize(width: 0, height: -18), blur: 40, color: color(0, 0, 0, 0.28))
ctx.addPath(squircle); ctx.setFillColor(color(0.1, 0.1, 0.2)); ctx.fillPath()
ctx.restoreGState()

// Glass body: deep night blue with two colored light pools, like the app background.
ctx.saveGState(); ctx.addPath(squircle); ctx.clip()
ctx.drawLinearGradient(gradient([color(0.07, 0.09, 0.22), color(0.11, 0.16, 0.36)], [0, 1]),
                       start: CGPoint(x: 512, y: 100), end: CGPoint(x: 512, y: 924), options: [])
ctx.drawRadialGradient(gradient([color(0.36, 0.42, 1.0, 0.85), color(0.36, 0.42, 1.0, 0)], [0, 1]),
                       startCenter: CGPoint(x: 300, y: 760), startRadius: 0, endCenter: CGPoint(x: 300, y: 760), endRadius: 520, options: [])
ctx.drawRadialGradient(gradient([color(0.15, 0.80, 0.85, 0.55), color(0.15, 0.80, 0.85, 0)], [0, 1]),
                       startCenter: CGPoint(x: 820, y: 230), startRadius: 0, endCenter: CGPoint(x: 820, y: 230), endRadius: 460, options: [])
// Top sheen.
ctx.drawLinearGradient(gradient([color(1, 1, 1, 0.16), color(1, 1, 1, 0)], [0, 1]),
                       start: CGPoint(x: 512, y: 924), end: CGPoint(x: 512, y: 560), options: [])
ctx.restoreGState()

let white = color(0.98, 0.99, 1)
let coral = color(1.0, 0.42, 0.38)
func glow(_ c: CGColor, _ blur: CGFloat, _ draw: () -> Void) {
    ctx.saveGState(); ctx.setShadow(offset: .zero, blur: blur, color: c); draw(); ctx.restoreGState()
}

switch variant {
case "thread":
    // The timeline itself: one thin line, three apps hanging, the present glowing.
    let lineY = 430.0
    glow(color(1, 1, 1, 0.5), 18) {
        ctx.setStrokeColor(white); ctx.setLineWidth(18); ctx.setLineCap(.round)
        ctx.move(to: CGPoint(x: 220, y: lineY)); ctx.addLine(to: CGPoint(x: 804, y: lineY)); ctx.strokePath()
    }
    for (x, s, a) in [(330.0, 92.0, 0.55), (512.0, 132.0, 1.0), (694.0, 92.0, 0.55)] {
        ctx.setStrokeColor(color(1, 1, 1, a)); ctx.setLineWidth(8)
        ctx.move(to: CGPoint(x: x, y: lineY)); ctx.addLine(to: CGPoint(x: x, y: lineY + 70)); ctx.strokePath()
        let rect = CGRect(x: x - s / 2, y: lineY + 70, width: s, height: s)
        glow(color(1, 1, 1, a * 0.6), 24) {
            ctx.addPath(CGPath(roundedRect: rect, cornerWidth: s * 0.24, cornerHeight: s * 0.24, transform: nil))
            ctx.setFillColor(color(1, 1, 1, a)); ctx.fillPath()
        }
    }
    glow(coral, 30) { ctx.setFillColor(coral); ctx.fillEllipse(in: CGRect(x: 580, y: lineY - 22, width: 44, height: 44)) }
case "orbit":
    // A persona (soft orb) with a path passing through it.
    glow(color(0.6, 0.7, 1, 0.8), 60) {
        ctx.drawRadialGradient(gradient([white, color(0.75, 0.82, 1, 0.9), color(0.5, 0.6, 1, 0)], [0, 0.55, 1]),
                               startCenter: CGPoint(x: 470, y: 560), startRadius: 0, endCenter: CGPoint(x: 512, y: 512), endRadius: 230, options: [])
    }
    ctx.saveGState()
    ctx.translateBy(x: 512, y: 512); ctx.rotate(by: -0.42)
    ctx.setStrokeColor(color(1, 1, 1, 0.9)); ctx.setLineWidth(14)
    ctx.addEllipse(in: CGRect(x: -330, y: -110, width: 660, height: 220)); ctx.strokePath()
    glow(coral, 30) { ctx.setFillColor(coral); ctx.fillEllipse(in: CGRect(x: 260, y: 40, width: 50, height: 50)) }
    ctx.restoreGState()
default:
    // "P" made of the day ring (bowl) and the timeline (stem); the coral dot is now.
    let center = CGPoint(x: 548, y: 600), radius = 168.0
    glow(color(1, 1, 1, 0.45), 22) {
        ctx.setStrokeColor(white); ctx.setLineWidth(62); ctx.setLineCap(.round)
        ctx.move(to: CGPoint(x: 380, y: 250)); ctx.addLine(to: CGPoint(x: 380, y: 600))
        ctx.addArc(center: center, radius: radius, startAngle: .pi, endAngle: -0.62, clockwise: true)
        ctx.strokePath()
    }
    let end = CGPoint(x: center.x + radius * cos(-0.62), y: center.y + radius * sin(-0.62))
    glow(coral, 36) { ctx.setFillColor(coral); ctx.fillEllipse(in: CGRect(x: end.x - 46, y: end.y - 46, width: 92, height: 92)) }
}

// Hairline rim so the glass reads on light and dark docks.
ctx.addPath(CGPath(roundedRect: tile.insetBy(dx: 2, dy: 2), cornerWidth: 183, cornerHeight: 183, transform: nil))
ctx.setStrokeColor(color(1, 1, 1, 0.18)); ctx.setLineWidth(4); ctx.strokePath()
NSGraphicsContext.restoreGraphicsState()
try bitmap.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: path))
