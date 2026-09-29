// Generates Workbench's app icons into public/icons/ from one geometry (the
// favicon's, on a 512 grid): the SVG (manifest, desktop launcher), PNGs for the
// web app manifest (192/512, maskable), the iOS home screen icon, the
// monochrome notification badge; and the Windows icon of workbench.exe and
// workbenchw.exe into packaging/windows/ (server/build.rs embeds it; kept out of the
// web bundle). No dependencies: shapes are rasterized with 8×8 supersampling and
// written as PNG with node:zlib.
//
//   node scripts/icons.mjs
//
// Colours are the dark theme tokens (theme/tokens.css): --bg-panel, --accent, --success.

import { mkdirSync, writeFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { deflateSync } from 'node:zlib'

const OUT = join(dirname(fileURLToPath(import.meta.url)), '..', 'public', 'icons')
const OUT_ICO = join(dirname(fileURLToPath(import.meta.url)), '..', '..', 'packaging', 'windows')

const BG = '#2b2d30' // --bg-panel (dark)
const ACCENT = '#548af7' // --accent (dark)
const SUCCESS = '#5fb865' // --success (dark)
const WHITE = '#ffffff'

// The glyph on a 512×512 canvas: three lines with round caps and a dot.
const LINE_WIDTH = 48
const LINES = [
  [112, 160, 400, 160],
  [112, 256, 288, 256],
  [112, 352, 336, 352],
]
const DOT = [384, 336, 48]
const RADIUS = 112 // the rounded square's corner radius

/** Shapes in paint order for one icon variant. `scale` shrinks the glyph around the centre. */
function shapes({ background, rounded, scale = 1, mono = false }) {
  const t = (v) => 256 + (v - 256) * scale
  const out = []
  if (background) out.push({ kind: 'rect', rx: rounded ? RADIUS : 0, color: BG })
  for (const [x1, y1, x2, y2] of LINES) out.push({ kind: 'line', x1: t(x1), y1: t(y1), x2: t(x2), y2: t(y2), w: LINE_WIDTH * scale, color: mono ? WHITE : ACCENT })
  out.push({ kind: 'circle', cx: t(DOT[0]), cy: t(DOT[1]), r: DOT[2] * scale, color: mono ? WHITE : SUCCESS })
  return out
}

function svg(list) {
  const body = list
    .map((s) => {
      if (s.kind === 'rect') return `<rect width="512" height="512" rx="${s.rx}" fill="${s.color}"/>`
      if (s.kind === 'line') return `<path d="M${s.x1} ${s.y1}H${s.x2}" stroke="${s.color}" stroke-width="${s.w}" stroke-linecap="round"/>`
      return `<circle cx="${s.cx}" cy="${s.cy}" r="${s.r}" fill="${s.color}"/>`
    })
    .join('')
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 512 512">${body}</svg>\n`
}

function inside(s, x, y) {
  if (s.kind === 'rect') {
    if (x < 0 || y < 0 || x > 512 || y > 512) return false
    const r = s.rx
    const cx = Math.min(Math.max(x, r), 512 - r)
    const cy = Math.min(Math.max(y, r), 512 - r)
    return (x - cx) ** 2 + (y - cy) ** 2 <= r * r
  }
  if (s.kind === 'line') {
    const dx = s.x2 - s.x1
    const dy = s.y2 - s.y1
    const k = Math.max(0, Math.min(1, ((x - s.x1) * dx + (y - s.y1) * dy) / (dx * dx + dy * dy)))
    return (x - (s.x1 + k * dx)) ** 2 + (y - (s.y1 + k * dy)) ** 2 <= (s.w / 2) ** 2
  }
  return (x - s.cx) ** 2 + (y - s.cy) ** 2 <= s.r * s.r
}

const rgb = (hex) => [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16) / 255)

/** RGBA pixels (premultiplied while compositing, straight in the output). */
function raster(list, size) {
  const SS = 8
  const px = Buffer.alloc(size * size * 4)
  const cols = list.map((s) => rgb(s.color))
  for (let py = 0; py < size; py++) {
    for (let pxx = 0; pxx < size; pxx++) {
      let r = 0
      let g = 0
      let b = 0
      let a = 0
      for (let sy = 0; sy < SS; sy++) {
        for (let sx = 0; sx < SS; sx++) {
          const x = ((pxx + (sx + 0.5) / SS) * 512) / size
          const y = ((py + (sy + 0.5) / SS) * 512) / size
          // Topmost opaque shape wins (all shapes are opaque).
          for (let i = list.length - 1; i >= 0; i--) {
            if (inside(list[i], x, y)) {
              r += cols[i][0]
              g += cols[i][1]
              b += cols[i][2]
              a += 1
              break
            }
          }
        }
      }
      const o = (py * size + pxx) * 4
      if (a) {
        px[o] = Math.round((r / a) * 255)
        px[o + 1] = Math.round((g / a) * 255)
        px[o + 2] = Math.round((b / a) * 255)
      }
      px[o + 3] = Math.round((a / (SS * SS)) * 255)
    }
  }
  return px
}

const CRC = new Uint32Array(256).map((_, n) => {
  let c = n
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1
  return c >>> 0
})
function crc32(buf) {
  let c = 0xffffffff
  for (const byte of buf) c = CRC[(c ^ byte) & 0xff] ^ (c >>> 8)
  return (c ^ 0xffffffff) >>> 0
}
function chunk(type, data) {
  const len = Buffer.alloc(4)
  len.writeUInt32BE(data.length)
  const td = Buffer.concat([Buffer.from(type, 'ascii'), data])
  const crc = Buffer.alloc(4)
  crc.writeUInt32BE(crc32(td))
  return Buffer.concat([len, td, crc])
}
function png(pixels, size) {
  const ihdr = Buffer.alloc(13)
  ihdr.writeUInt32BE(size, 0)
  ihdr.writeUInt32BE(size, 4)
  ihdr[8] = 8 // bit depth
  ihdr[9] = 6 // RGBA
  const raw = Buffer.alloc(size * (size * 4 + 1))
  for (let y = 0; y < size; y++) {
    raw[y * (size * 4 + 1)] = 0 // filter: none
    pixels.copy(raw, y * (size * 4 + 1) + 1, y * size * 4, (y + 1) * size * 4)
  }
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk('IHDR', ihdr),
    chunk('IDAT', deflateSync(raw, { level: 9 })),
    chunk('IEND', Buffer.alloc(0)),
  ])
}

/** An .ico of PNG images (read by Windows Vista and later), one per `[size, png]`. */
function ico(images) {
  const header = Buffer.alloc(6)
  header.writeUInt16LE(1, 2) // type: icon
  header.writeUInt16LE(images.length, 4)
  let offset = header.length + 16 * images.length
  const entries = images.map(([size, data]) => {
    const e = Buffer.alloc(16)
    e[0] = size % 256 // width; 0 means 256
    e[1] = size % 256 // height
    e.writeUInt16LE(1, 4) // colour planes
    e.writeUInt16LE(32, 6) // bits per pixel
    e.writeUInt32LE(data.length, 8)
    e.writeUInt32LE(offset, 12)
    offset += data.length
    return e
  })
  return Buffer.concat([header, ...entries, ...images.map(([, data]) => data)])
}

const icon = shapes({ background: true, rounded: true })
// Maskable: full bleed, the glyph inside the 80% safe circle.
const maskable = shapes({ background: true, rounded: false, scale: 0.78 })
// iOS masks the corners itself and shows transparency as black.
const apple = shapes({ background: true, rounded: false, scale: 0.86 })
// Android status bar badge: only the alpha channel counts.
const badge = shapes({ background: false, rounded: false, scale: 1.12, mono: true })

mkdirSync(OUT, { recursive: true })
writeFileSync(join(OUT, 'workbench.svg'), svg(icon))
const files = [
  ['icon-192.png', icon, 192],
  ['icon-512.png', icon, 512],
  ['maskable-192.png', maskable, 192],
  ['maskable-512.png', maskable, 512],
  ['apple-touch-icon.png', apple, 180],
  ['badge-96.png', badge, 96],
]
for (const [name, list, size] of files) writeFileSync(join(OUT, name), png(raster(list, size), size))
// Windows: small icons at 100–250 % scaling, the Start Menu's and Explorer's larger ones.
const icoSizes = [16, 20, 24, 32, 40, 48, 64, 256]
mkdirSync(OUT_ICO, { recursive: true })
writeFileSync(join(OUT_ICO, 'workbench.ico'), ico(icoSizes.map((s) => [s, png(raster(icon, s), s)])))
console.log(`wrote workbench.svg and ${files.length} PNGs to ${OUT}, workbench.ico to ${OUT_ICO}`)
