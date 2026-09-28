/**
 * Minimal imperative three.js model viewer for side-by-side comparisons.
 * Adapted from Mr. Mak Workspace (MIT), src/lib/three/viewer.ts.
 *
 *   • every model is normalised into the same 2-unit box, so two panes compare honestly
 *   • shading set: wireframe over a dark body (quads when the file declares
 *     FB_ngon_encoding), clay, geometry normals, textured PBR, raw texture channels
 *   • one shared Draco decoder, served from our own origin (the JS build: the app's
 *     CSP allows no WebAssembly compilation)
 *   • camera state in/out so panes can be locked together
 *
 * Everything is disposed on unmount. UI colours come from the theme tokens.
 */
import * as THREE from 'three'
import { GLTFLoader, type GLTFLoaderPlugin, type GLTFParser } from 'three/examples/jsm/loaders/GLTFLoader.js'
import { DRACOLoader } from 'three/examples/jsm/loaders/DRACOLoader.js'
import { OrbitControls } from 'three/examples/jsm/controls/OrbitControls.js'
import { RoomEnvironment } from 'three/examples/jsm/environments/RoomEnvironment.js'
import dracoDecoderUrl from 'three/examples/jsm/libs/draco/draco_decoder.js?url'
import { SHADING_MODES, type ShadingModeId } from '../logic'

export type ShadingMode = ShadingModeId
const KNOWN_MODES = new Set<string>(SHADING_MODES)

export interface ViewerStats {
  vertices: number
  triangles: number
  meshes: number
  materials: number
  hasNormalMap: boolean
  hasRoughMap: boolean
  hasMetalMap: boolean
  hasBaseMap: boolean
  /** Largest texture dimension in pixels; 0 when untextured. */
  textureSize: number
}

export interface CameraState {
  pos: [number, number, number]
  target: [number, number, number]
}

/**
 * Recover polygon outlines from a triangulated glTF. With FB_ngon_encoding the
 * triangles of one polygon are consecutive and share their first index; within a
 * group, an edge used once is outline, an edge used twice an internal diagonal.
 */
function ngonGroups(geo: THREE.BufferGeometry): number[] {
  const index = geo.index
  const pos = geo.attributes.position
  const triCount = index ? index.count / 3 : (pos?.count ?? 0) / 3
  const at = index ? (i: number) => index.getX(i) : (i: number) => i
  const sizes: number[] = []
  let i = 0
  while (i < triCount) {
    const first = at(i * 3)
    let j = i + 1
    while (j < triCount && at(j * 3) === first) j++
    sizes.push(j - i)
    i = j
  }
  return sizes
}

function ngonEdgeGeometry(geo: THREE.BufferGeometry): THREE.BufferGeometry {
  const index = geo.index
  const pos = geo.attributes.position as THREE.BufferAttribute
  const at = index ? (i: number) => index.getX(i) : (i: number) => i
  const out: number[] = []
  const ea: number[] = []
  const eb: number[] = []
  const used: number[] = []
  let tri = 0
  for (const size of ngonGroups(geo)) {
    ea.length = eb.length = used.length = 0
    for (let t = tri; t < tri + size; t++) {
      const a = at(t * 3)
      const b = at(t * 3 + 1)
      const c = at(t * 3 + 2)
      for (const [u, v] of [
        [a, b],
        [b, c],
        [c, a],
      ] as [number, number][]) {
        const lo = Math.min(u, v)
        const hi = Math.max(u, v)
        let found = -1
        for (let k = 0; k < ea.length; k++) {
          if (ea[k] === lo && eb[k] === hi) {
            found = k
            break
          }
        }
        if (found >= 0) used[found]++
        else {
          ea.push(lo)
          eb.push(hi)
          used.push(1)
        }
      }
    }
    for (let k = 0; k < ea.length; k++) {
      if (used[k] !== 1) continue
      out.push(pos.getX(ea[k]), pos.getY(ea[k]), pos.getZ(ea[k]), pos.getX(eb[k]), pos.getY(eb[k]), pos.getZ(eb[k]))
    }
    tri += size
  }
  const g = new THREE.BufferGeometry()
  g.setAttribute('position', new THREE.Float32BufferAttribute(out, 3))
  return g
}

/** One Draco decoder for every pane: two panes decode at once. */
let sharedDraco: DRACOLoader | null = null
function draco(): DRACOLoader {
  if (!sharedDraco) {
    const d = new DRACOLoader() as DRACOLoader & { decoderPaths: { dep_js: string | null }; decoderConfig: Record<string, unknown> }
    // The asm.js decoder: WebAssembly would need 'wasm-unsafe-eval' in the app's CSP.
    d.decoderPaths.dep_js = dracoDecoderUrl
    d.decoderConfig = { type: 'js' }
    d.setWorkerLimit(2)
    sharedDraco = d
  }
  return sharedDraco
}

/**
 * Decode textures through `<img>`, not ImageBitmapLoader: that one `fetch()`es the
 * blob: (or data:) URL of an embedded image, which the app's CSP refuses
 * (`connect-src 'self'`), and the model would render untextured. `img-src` allows
 * blob: and data:. GLTFLoader plugins are created before any texture loads, and
 * it sets `flipY = false` for either loader.
 */
function imageTextures(parser: GLTFParser): GLTFLoaderPlugin {
  const loader = new THREE.TextureLoader(parser.options.manager)
  loader.setCrossOrigin(parser.options.crossOrigin)
  loader.setRequestHeader(parser.options.requestHeader)
  parser.textureLoader = loader
  return { name: 'workbench_image_textures' }
}

/** A theme colour (CSS custom property) as a three.js colour. */
function themeColor(name: string, fallback: THREE.Color): THREE.Color {
  const v = getComputedStyle(document.documentElement).getPropertyValue(name).trim()
  try {
    return v ? new THREE.Color(v) : fallback
  } catch {
    return fallback
  }
}

const WHITE = new THREE.Color(1, 1, 1)
const GREY = new THREE.Color(0.76, 0.76, 0.78)
const NEAR_BLACK = new THREE.Color(0.04, 0.04, 0.05)
/** A flat tangent-space normal (0.5, 0.5, 1): what "no normal map" looks like. */
const FLAT_NORMAL = new THREE.Color(0.5, 0.5, 1)

export class ModelViewer {
  readonly renderer: THREE.WebGLRenderer
  readonly scene = new THREE.Scene()
  readonly camera: THREE.PerspectiveCamera
  readonly controls: OrbitControls

  /** Fired whenever the user moves this camera (pane-to-pane sync). */
  onCameraChange: ((s: CameraState) => void) | null = null
  onStats: ((s: ViewerStats) => void) | null = null

  private container: HTMLElement
  private pmrem: THREE.PMREMGenerator
  private envRT: THREE.WebGLRenderTarget | null = null
  private root = new THREE.Group()
  private lightsPbr = new THREE.Group()
  private lightsClay = new THREE.Group()
  private originalMaterials = new Map<THREE.Mesh, THREE.Material | THREE.Material[]>()
  private ownedMaterials = new Set<THREE.Material>()
  private wireOverlays = new Set<THREE.LineSegments>()
  /** Wireframe geometry per mesh, built once (rebuilding it on a big mesh is not free). */
  private ngonEdges = new Map<THREE.Mesh, THREE.BufferGeometry>()
  private ngonDeclared = false
  private ro: ResizeObserver
  private raf = 0
  private disposed = false
  private applying = false
  private mode: ShadingMode = 'solid'
  private clay: THREE.Color
  private wireBody: THREE.Color
  private wireLine: THREE.Color

  constructor(container: HTMLElement) {
    this.container = container
    this.clay = themeColor('--fg-muted', GREY)
    this.wireBody = themeColor('--bg', NEAR_BLACK)
    this.wireLine = themeColor('--fg', WHITE)

    this.renderer = new THREE.WebGLRenderer({ antialias: true, alpha: true, powerPreference: 'high-performance', preserveDrawingBuffer: false })
    this.renderer.setPixelRatio(Math.min(window.devicePixelRatio, 2))
    this.renderer.setClearColor(NEAR_BLACK, 0)
    this.renderer.outputColorSpace = THREE.SRGBColorSpace
    this.renderer.toneMapping = THREE.ACESFilmicToneMapping
    this.renderer.toneMappingExposure = 1.05
    container.appendChild(this.renderer.domElement)

    this.camera = new THREE.PerspectiveCamera(45, 1, 0.01, 200)
    this.camera.position.set(0, 0.4, 4)

    this.controls = new OrbitControls(this.camera, this.renderer.domElement)
    this.controls.enableDamping = true
    this.controls.dampingFactor = 0.08
    this.controls.minDistance = 0.4
    this.controls.maxDistance = 40
    this.controls.autoRotateSpeed = 0.9
    this.controls.addEventListener('change', () => {
      if (this.applying || !this.onCameraChange) return
      this.onCameraChange(this.cameraState())
    })
    // A manual grab stops auto-rotate.
    this.controls.addEventListener('start', () => this.setAutoRotate(false))

    this.pmrem = new THREE.PMREMGenerator(this.renderer)

    this.lightsClay.add(new THREE.HemisphereLight(WHITE, new THREE.Color(0.1, 0.1, 0.125), 1.15))
    const key = new THREE.DirectionalLight(WHITE, 1.1)
    key.position.set(4, 7, 5)
    this.lightsClay.add(key)
    const fill = new THREE.DirectionalLight(WHITE, 0.35)
    fill.position.set(-5, 2, -4)
    this.lightsClay.add(fill)

    this.lightsPbr.add(new THREE.AmbientLight(WHITE, 0.35))
    const pk = new THREE.DirectionalLight(WHITE, 1.1)
    pk.position.set(6, 8, 5)
    this.lightsPbr.add(pk)
    const pr = new THREE.DirectionalLight(WHITE, 0.4)
    pr.position.set(-6, -3, -5)
    this.lightsPbr.add(pr)

    this.scene.add(this.root)
    this.ro = new ResizeObserver(() => this.resize())
    this.ro.observe(container)
    this.resize()
    this.tick()
  }

  /** Load a .glb/.gltf; `rotationY` (degrees) fixes exports that face the wrong way. */
  async load(url: string, rotationY = 0): Promise<void> {
    const loader = new GLTFLoader()
    loader.setDRACOLoader(draco())
    loader.register(imageTextures)
    const gltf = await loader.loadAsync(url)
    if (this.disposed) return
    this.clearModel()

    const declared: string[] = (gltf.parser as unknown as { json?: { extensionsUsed?: string[] } })?.json?.extensionsUsed ?? []
    this.ngonDeclared = declared.includes('FB_ngon_encoding')

    const model = gltf.scene
    // Yaw first, so the box (and the centring) is measured as shown. Scale the
    // longest axis to 2 units, then measure again and centre on that.
    if (rotationY) model.rotation.y += (rotationY * Math.PI) / 180
    model.updateMatrixWorld(true)
    const size = new THREE.Box3().setFromObject(model).getSize(new THREE.Vector3())
    const maxDim = Math.max(size.x, size.y, size.z) || 1
    model.scale.setScalar(2 / maxDim)
    model.updateMatrixWorld(true)
    const centre = new THREE.Box3().setFromObject(model).getCenter(new THREE.Vector3())
    model.position.sub(centre)
    this.root.add(model)

    let vertices = 0
    let triangles = 0
    let meshes = 0
    const mats = new Set<THREE.Material>()
    let hasNormalMap = false
    let hasRoughMap = false
    let hasMetalMap = false
    let hasBaseMap = false
    let textureSize = 0
    model.traverse((o) => {
      const mesh = o as THREE.Mesh
      if (!mesh.isMesh) return
      meshes++
      const geo = mesh.geometry as THREE.BufferGeometry
      if (!geo.attributes.normal) geo.computeVertexNormals()
      const pos = geo.attributes.position
      if (pos) vertices += pos.count
      triangles += geo.index ? geo.index.count / 3 : (pos?.count ?? 0) / 3
      this.originalMaterials.set(mesh, mesh.material)
      for (const m of Array.isArray(mesh.material) ? mesh.material : [mesh.material]) {
        if (!m) continue
        mats.add(m)
        const s = m as THREE.MeshStandardMaterial
        if (s.map) hasBaseMap = true
        if (s.normalMap) hasNormalMap = true
        if (s.roughnessMap) hasRoughMap = true
        if (s.metalnessMap) hasMetalMap = true
        for (const t of [s.map, s.normalMap, s.roughnessMap, s.metalnessMap]) {
          const img = t?.image as { width?: number; height?: number } | undefined
          if (img?.width) textureSize = Math.max(textureSize, img.width, img.height ?? 0)
        }
      }
    })
    this.onStats?.({
      vertices: Math.round(vertices),
      triangles: Math.round(triangles),
      meshes,
      materials: mats.size,
      hasBaseMap,
      hasNormalMap,
      hasRoughMap,
      hasMetalMap,
      textureSize,
    })
    this.frame()
    this.setMode(this.mode)
  }

  setMode(requested: ShadingMode): void {
    // An unknown mode (from a manifest) would get no lights: a black model.
    const mode: ShadingMode = KNOWN_MODES.has(requested) ? requested : 'solid'
    this.mode = mode
    this.releaseOwned()
    if (mode === 'pbr' && !this.envRT) {
      // A procedural room instead of an HDRI: no network fetch.
      const room = new RoomEnvironment()
      this.envRT = this.pmrem.fromScene(room, 0.04)
      room.dispose()
    }
    this.scene.environment = mode === 'pbr' ? (this.envRT?.texture ?? null) : null
    this.lightsPbr.removeFromParent()
    this.lightsClay.removeFromParent()
    if (mode === 'pbr') this.scene.add(this.lightsPbr)
    else if (mode === 'solid' || mode === 'wire') this.scene.add(this.lightsClay)

    for (const [mesh, original] of this.originalMaterials) {
      const first = (Array.isArray(original) ? original[0] : original) as THREE.MeshStandardMaterial
      const basic = (p: THREE.MeshBasicMaterialParameters) => this.own(new THREE.MeshBasicMaterial({ side: THREE.DoubleSide, ...p }))
      switch (mode) {
        case 'pbr':
          mesh.material = original
          break
        case 'solid':
          mesh.material = this.own(new THREE.MeshStandardMaterial({ color: this.clay, metalness: 0, roughness: 0.85, side: THREE.DoubleSide }))
          break
        case 'normals':
          mesh.material = this.own(new THREE.MeshNormalMaterial({ side: THREE.DoubleSide }))
          break
        case 'wire': {
          // A dark unlit body so the edges are all you read; pushed back a hair so
          // the lines sit on top without z-fighting, and still hides far edges.
          mesh.material = basic({ color: this.wireBody, polygonOffset: true, polygonOffsetFactor: 1, polygonOffsetUnits: 1 })
          let wireGeo = this.ngonEdges.get(mesh)
          if (!wireGeo) {
            wireGeo = this.ngonDeclared ? ngonEdgeGeometry(mesh.geometry as THREE.BufferGeometry) : new THREE.WireframeGeometry(mesh.geometry)
            this.ngonEdges.set(mesh, wireGeo)
          }
          const wf = new THREE.LineSegments(wireGeo, this.own(new THREE.LineBasicMaterial({ color: this.wireLine })))
          wf.renderOrder = (mesh.renderOrder || 0) + 1
          mesh.add(wf)
          this.wireOverlays.add(wf)
          break
        }
        // Raw channels, unlit: a flat or missing map is obvious.
        case 'albedo':
          mesh.material = basic({ map: first?.map ?? null, color: first?.map ? WHITE : (first?.color ?? WHITE) })
          break
        case 'normalMap':
          mesh.material = first?.normalMap ? basic({ map: first.normalMap }) : basic({ color: FLAT_NORMAL })
          break
        case 'rough':
          mesh.material = first?.roughnessMap ? basic({ map: first.roughnessMap }) : basic({ color: new THREE.Color().setScalar(first?.roughness ?? 1) })
          break
        case 'metal':
          mesh.material = first?.metalnessMap ? basic({ map: first.metalnessMap }) : basic({ color: new THREE.Color().setScalar(first?.metalness ?? 0) })
          break
      }
    }
  }

  /** Frame the (normalised) model. Does not broadcast: a pane that loads later must
   *  not yank a camera already set by hand. */
  frame(): void {
    const box = new THREE.Box3().setFromObject(this.root)
    if (box.isEmpty()) return
    this.applying = true
    const size = box.getSize(new THREE.Vector3())
    const centre = box.getCenter(new THREE.Vector3())
    const maxDim = Math.max(size.x, size.y, size.z) || 1
    // Fit both ways: a narrow pane (phone, three models side by side) is limited by
    // its horizontal field of view.
    const vfov = (this.camera.fov * Math.PI) / 180
    const hfov = 2 * Math.atan(Math.tan(vfov / 2) * this.camera.aspect)
    const dist = (maxDim / 2 / Math.tan(Math.min(vfov, hfov) / 2)) * 1.6
    this.camera.position.set(centre.x, centre.y + size.y * 0.12, centre.z + dist)
    this.controls.target.copy(centre)
    this.controls.update()
    this.applying = false
  }

  cameraState(): CameraState {
    const p = this.camera.position
    const t = this.controls.target
    return { pos: [p.x, p.y, p.z], target: [t.x, t.y, t.z] }
  }

  /** Apply another pane's camera without echoing a change back. */
  applyCamera(s: CameraState): void {
    this.applying = true
    this.camera.position.set(...s.pos)
    this.controls.target.set(...s.target)
    this.controls.update()
    this.applying = false
  }

  setAutoRotate(on: boolean): void {
    this.controls.autoRotate = on
  }

  private own<T extends THREE.Material>(m: T): T {
    this.ownedMaterials.add(m)
    return m
  }

  /** Drop what this viewer created; never loaded assets. Cached outlines stay. */
  private releaseOwned(): void {
    const cached = new Set(this.ngonEdges.values())
    for (const wf of this.wireOverlays) {
      wf.removeFromParent()
      if (!cached.has(wf.geometry)) wf.geometry.dispose()
    }
    this.wireOverlays.clear()
    for (const m of this.ownedMaterials) m.dispose()
    this.ownedMaterials.clear()
  }

  private clearModel(): void {
    this.releaseOwned()
    for (const g of this.ngonEdges.values()) g.dispose()
    this.ngonEdges.clear()
    this.originalMaterials.clear()
    for (const child of this.root.children.slice()) {
      this.root.remove(child)
      child.traverse((o) => {
        const mesh = o as THREE.Mesh
        if (!mesh.isMesh) return
        mesh.geometry?.dispose()
        for (const m of Array.isArray(mesh.material) ? mesh.material : [mesh.material]) {
          if (!m) continue
          for (const v of Object.values(m as unknown as Record<string, unknown>)) {
            if (v && (v as THREE.Texture).isTexture) (v as THREE.Texture).dispose()
          }
          m.dispose()
        }
      })
    }
  }

  private resize(): void {
    const w = this.container.clientWidth || 1
    const h = this.container.clientHeight || 1
    this.renderer.setSize(w, h, false)
    this.camera.aspect = w / h
    this.camera.updateProjectionMatrix()
  }

  private tick = (): void => {
    if (this.disposed) return
    this.raf = requestAnimationFrame(this.tick)
    this.controls.update()
    this.renderer.render(this.scene, this.camera)
  }

  dispose(): void {
    this.disposed = true
    cancelAnimationFrame(this.raf)
    this.ro.disconnect()
    this.controls.dispose()
    this.clearModel()
    this.envRT?.dispose()
    this.pmrem.dispose()
    this.renderer.dispose()
    this.renderer.domElement.remove()
  }
}
