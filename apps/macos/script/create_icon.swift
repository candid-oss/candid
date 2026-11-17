// Reproducible Candid app icon, drawn from code; no external image assets.
// usage: create_icon.swift OUTPUT.png
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

// Candid's mark on a flat ink tile: an open "c" (candor: nothing hidden) and, in its opening, the dot that is
// Candid's capture state everywhere (coral = recording). Flat colors, no gloss, so it reads at 16 px.
_ = variant
ctx.saveGState()
ctx.setShadow(offset: CGSize(width: 0, height: -14), blur: 34, color: color(0, 0, 0, 0.25))
ctx.addPath(squircle); ctx.setFillColor(color(0.086, 0.094, 0.114)); ctx.fillPath()
ctx.restoreGState()
let paper = color(0.957, 0.949, 0.933)
let coral = color(1.0, 0.42, 0.29)
let center = CGPoint(x: 492, y: 512), radius = 228.0, gap = 0.62
ctx.setStrokeColor(paper); ctx.setLineWidth(76); ctx.setLineCap(.round)
ctx.addArc(center: center, radius: radius, startAngle: gap, endAngle: -gap, clockwise: false)
ctx.strokePath()
ctx.setFillColor(coral)
ctx.fillEllipse(in: CGRect(x: center.x + radius - 44, y: center.y - 52, width: 104, height: 104))
// Hairline rim so the tile reads on dark docks.
ctx.addPath(CGPath(roundedRect: tile.insetBy(dx: 2, dy: 2), cornerWidth: 183, cornerHeight: 183, transform: nil))
ctx.setStrokeColor(color(1, 1, 1, 0.10)); ctx.setLineWidth(4); ctx.strokePath()
NSGraphicsContext.restoreGraphicsState()
try bitmap.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: path))
