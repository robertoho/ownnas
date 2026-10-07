import * as THREE from "/assets/vendor/three/three.module.min.js";
import { OrbitControls } from "/assets/vendor/three/addons/controls/OrbitControls.js";
import { OBJLoader } from "/assets/vendor/three/addons/loaders/OBJLoader.js";
import { STLLoader } from "/assets/vendor/three/addons/loaders/STLLoader.js";
import { GLTFLoader } from "/assets/vendor/three/addons/loaders/GLTFLoader.js";
import { PLYLoader } from "/assets/vendor/three/addons/loaders/PLYLoader.js";
import { ThreeMFLoader } from "/assets/vendor/three/addons/loaders/3MFLoader.js";

let active = null;

function extensionOf(name) {
  const lower = String(name || "").toLowerCase();
  const dot = lower.lastIndexOf(".");
  return dot >= 0 ? lower.slice(dot + 1) : "";
}

export function isCadModel(name) {
  return ["stp", "step", "iges", "igs"].includes(extensionOf(name));
}

/** @deprecated kept for callers; CAD formats are now viewable */
export function isCadOnlyModel(name) {
  return false;
}

export function isViewableModel(name) {
  return ["obj", "stl", "gltf", "glb", "ply", "3mf", "stp", "step", "iges", "igs"].includes(
    extensionOf(name),
  );
}

function themeColors() {
  const styles = getComputedStyle(document.documentElement);
  const bg = styles.getPropertyValue("--bg").trim() || "#1a1f18";
  const accent = styles.getPropertyValue("--accent").trim() || "#9bbf7a";
  const muted = styles.getPropertyValue("--muted").trim() || "#8a9380";
  const line = styles.getPropertyValue("--line").trim() || "#2f382c";
  return { bg, accent, muted, line };
}

function disposeObject(root) {
  if (!root) return;
  root.traverse((obj) => {
    if (obj.geometry) obj.geometry.dispose();
    const mats = obj.material;
    if (!mats) return;
    const list = Array.isArray(mats) ? mats : [mats];
    for (const mat of list) {
      if (!mat) continue;
      for (const key of Object.keys(mat)) {
        const value = mat[key];
        if (value && value.isTexture) value.dispose();
      }
      mat.dispose();
    }
  });
}

export function disposeModelViewer() {
  if (!active) return;
  const session = active;
  active = null;
  session.alive = false;
  if (session.raf) cancelAnimationFrame(session.raf);
  if (session.ro) session.ro.disconnect();
  if (session.controls) session.controls.dispose();
  disposeObject(session.root);
  if (session.renderer) {
    session.renderer.dispose();
    session.renderer.forceContextLoss?.();
  }
  if (session.wrap && session.wrap.parentNode) {
    session.wrap.remove();
  }
}

function fitCamera(camera, controls, object) {
  const box = new THREE.Box3().setFromObject(object);
  if (box.isEmpty()) return;
  const size = box.getSize(new THREE.Vector3());
  const center = box.getCenter(new THREE.Vector3());
  const maxDim = Math.max(size.x, size.y, size.z, 0.001);
  const fov = (camera.fov * Math.PI) / 180;
  let distance = (maxDim / (2 * Math.tan(fov / 2))) * 1.45;
  distance = Math.max(distance, maxDim * 0.9);
  camera.near = Math.max(distance / 200, 0.01);
  camera.far = Math.max(distance * 100, 100);
  camera.updateProjectionMatrix();
  camera.position.set(center.x + distance * 0.65, center.y + distance * 0.45, center.z + distance * 0.85);
  controls.target.copy(center);
  controls.minDistance = maxDim * 0.15;
  controls.maxDistance = distance * 8;
  controls.update();
}

function defaultMaterial(colors) {
  return new THREE.MeshStandardMaterial({
    color: new THREE.Color(colors.accent),
    metalness: 0.15,
    roughness: 0.55,
    side: THREE.DoubleSide,
  });
}

function prepareRoot(object, colors) {
  const root = object.isObject3D ? object : new THREE.Mesh(object, defaultMaterial(colors));
  root.traverse((child) => {
    if (!child.isMesh) return;
    child.castShadow = false;
    child.receiveShadow = false;
    if (!child.material) {
      child.material = defaultMaterial(colors);
      return;
    }
    const mats = Array.isArray(child.material) ? child.material : [child.material];
    for (const mat of mats) {
      if (mat && mat.side === undefined) mat.side = THREE.DoubleSide;
    }
  });
  return root;
}

async function loadCadModel(url, ext, colors, onStatus, timeoutMs = 120000) {
  onStatus?.("Downloading CAD file…");
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), timeoutMs);
  let buffer;
  try {
    const response = await fetch(url, { credentials: "same-origin", signal: controller.signal });
    if (!response.ok) throw new Error(`Could not download model (${response.status})`);
    buffer = new Uint8Array(await response.arrayBuffer());
  } finally { clearTimeout(timeout); }
  const format = ext === "iges" || ext === "igs" ? "iges" : "step";
  onStatus?.("Tessellating with OpenCascade…");
  const result = await runOcctImport(format, buffer, timeoutMs);
  if (!result || !result.success) {
    throw new Error("OpenCascade could not import this CAD file");
  }
  const meshes = result.meshes || [];
  if (!meshes.length) throw new Error("CAD file imported with no mesh geometry");
  const group = new THREE.Group();
  for (const meshData of meshes) {
    const pos = meshData.attributes && meshData.attributes.position && meshData.attributes.position.array;
    if (!pos || !pos.length) continue;
    const geometry = new THREE.BufferGeometry();
    geometry.setAttribute("position", new THREE.BufferAttribute(new Float32Array(pos), 3));
    const norm = meshData.attributes.normal && meshData.attributes.normal.array;
    if (norm && norm.length) {
      geometry.setAttribute("normal", new THREE.BufferAttribute(new Float32Array(norm), 3));
    } else {
      geometry.computeVertexNormals();
    }
    if (meshData.index && meshData.index.array && meshData.index.array.length) {
      const idx = meshData.index.array;
      let maxIndex = 0;
      for (let i = 0; i < idx.length; i++) {
        if (idx[i] > maxIndex) maxIndex = idx[i];
      }
      geometry.setIndex(
        maxIndex > 65535
          ? new THREE.BufferAttribute(new Uint32Array(idx), 1)
          : new THREE.BufferAttribute(new Uint16Array(idx), 1),
      );
    }
    let matColor = colors.accent;
    if (meshData.color && meshData.color.length >= 3) {
      matColor = new THREE.Color(meshData.color[0], meshData.color[1], meshData.color[2]);
    }
    const material = new THREE.MeshStandardMaterial({
      color: matColor,
      metalness: 0.18,
      roughness: 0.52,
      side: THREE.DoubleSide,
    });
    const mesh = new THREE.Mesh(geometry, material);
    if (meshData.name) mesh.name = String(meshData.name);
    group.add(mesh);
  }
  if (!group.children.length) throw new Error("CAD file produced empty geometry");
  return prepareRoot(group, colors);
}

function runOcctImport(format, buffer, timeoutMs = 120000) {
  return new Promise((resolve, reject) => {
    let worker;
    try {
      worker = new Worker("/assets/vendor/occt/ownnas-occt-worker.js?v=2");
    } catch (err) {
      reject(err);
      return;
    }
    const timeout = setTimeout(() => {
      worker.terminate();
      reject(new Error("CAD import timed out"));
    }, timeoutMs);
    worker.onmessage = (ev) => {
      clearTimeout(timeout);
      worker.terminate();
      const data = ev.data || {};
      if (!data.ok) {
        reject(new Error(data.error || "CAD import failed"));
        return;
      }
      resolve(data.result);
    };
    worker.onerror = (err) => {
      clearTimeout(timeout);
      worker.terminate();
      reject(new Error(err && err.message ? err.message : "CAD worker failed"));
    };
    worker.postMessage({ format, buffer, params: null }, [buffer.buffer]);
  });
}

async function loadModel(url, ext, colors, onStatus, timeoutMs = 120000) {
  if (["stp", "step", "iges", "igs"].includes(ext)) {
    return loadCadModel(url, ext, colors, onStatus, timeoutMs);
  }
  const manager = new THREE.LoadingManager();
  const rawUrl = new URL(url, location.href);
  const modelPath = rawUrl.searchParams.get("path") || "";
  const directory = modelPath.includes("/") ? modelPath.slice(0, modelPath.lastIndexOf("/") + 1) : "";
  manager.setURLModifier((resource) => {
    if (resource.startsWith("ownnas-model-resource:/")) {
      const relative = resource.slice("ownnas-model-resource:/".length);
      const resolved = new URL(relative, `https://ownnas.invalid/${directory}`);
      if (resolved.origin !== "https://ownnas.invalid") throw new Error("External model resources are not supported");
      return `/api/raw?path=${encodeURIComponent(decodeURIComponent(resolved.pathname.slice(1)))}`;
    }
    if (resource.startsWith("data:") || resource.startsWith("blob:")) return resource;
    // GLTF may supply absolute resource URLs instead of applying the resource path.
    const resolved = new URL(resource, location.href);
    if (resolved.origin !== location.origin) throw new Error("External model resources are not supported");
    return resource;
  });
  const loaderFor = {
    obj: () => new OBJLoader(manager),
    stl: () => new STLLoader(manager),
    gltf: () => new GLTFLoader(manager),
    glb: () => new GLTFLoader(manager),
    ply: () => new PLYLoader(manager),
    "3mf": () => new ThreeMFLoader(manager),
  };
  const make = loaderFor[ext];
  if (!make) throw new Error(`Unsupported 3D format: .${ext}`);
  onStatus?.("Loading model…");
  const loader = make();
  if (ext === "gltf" || ext === "glb") loader.setResourcePath("ownnas-model-resource:/");
  const result = await new Promise((resolve, reject) => {
    let finished = false;
    const timer = setTimeout(() => { finished = true; reject(new Error("Model loading timed out")); }, timeoutMs);
    const loaded = (result) => {
      if (finished) {
        if (result.isBufferGeometry) result.dispose();
        else disposeObject(result.scene || result);
        return;
      }
      finished = true;
      clearTimeout(timer);
      resolve(result);
    };
    const failed = (err) => {
      if (finished) return;
      finished = true;
      clearTimeout(timer);
      reject(err);
    };
    try { loader.load(url, loaded, undefined, failed); }
    catch (err) { failed(err); }
  });
  if (ext === "gltf" || ext === "glb") {
    return prepareRoot(result.scene || result, colors);
  }
  if (ext === "stl" || ext === "ply") {
    const geometry = result;
    if (!geometry.getAttribute("normal")) geometry.computeVertexNormals();
    return prepareRoot(new THREE.Mesh(geometry, defaultMaterial(colors)), colors);
  }
  return prepareRoot(result, colors);
}

function setStatus(el, text, isError) {
  if (!el) return;
  el.hidden = !text;
  el.textContent = text || "";
  el.classList.toggle("is-error", !!isError);
}

function resize(session) {
  if (!session || !session.alive || !session.canvasHost) return;
  const rect = session.canvasHost.getBoundingClientRect();
  const width = Math.max(1, Math.floor(rect.width));
  const height = Math.max(1, Math.floor(rect.height));
  const dpr = Math.min(window.devicePixelRatio || 1, 2);
  session.renderer.setPixelRatio(dpr);
  session.renderer.setSize(width, height, false);
  session.camera.aspect = width / height;
  session.camera.updateProjectionMatrix();
}

export async function mountModelViewer(container, { url, name }) {
  disposeModelViewer();
  const ext = extensionOf(name);
  if (!isViewableModel(name)) {
    throw new Error("This 3D format cannot be previewed in the browser");
  }

  const colors = themeColors();
  const wrap = document.createElement("div");
  wrap.className = "model-viewer";
  const loadingLabel = isCadModel(name) ? "Preparing CAD viewer…" : "Loading model…";
  wrap.innerHTML = `
    <div class="model-viewer-toolbar">
      <button type="button" data-model="reset" title="Reset view">Reset</button>
      <button type="button" data-model="wire" aria-pressed="false" title="Toggle wireframe">Wireframe</button>
      <span class="model-viewer-hint">Drag to orbit · scroll to zoom · right-drag to pan</span>
    </div>
    <div class="model-viewer-canvas"></div>
    <p class="model-viewer-status">${loadingLabel}</p>
  `;
  container.innerHTML = "";
  container.appendChild(wrap);

  const canvasHost = wrap.querySelector(".model-viewer-canvas");
  const status = wrap.querySelector(".model-viewer-status");
  const resetBtn = wrap.querySelector('[data-model="reset"]');
  const wireBtn = wrap.querySelector('[data-model="wire"]');

  const scene = new THREE.Scene();
  scene.background = new THREE.Color(colors.bg);

  const camera = new THREE.PerspectiveCamera(45, 1, 0.1, 2000);
  camera.position.set(2.5, 1.8, 3.2);

  const renderer = new THREE.WebGLRenderer({ antialias: true, alpha: false });
  renderer.outputColorSpace = THREE.SRGBColorSpace;
  renderer.setClearColor(new THREE.Color(colors.bg), 1);
  canvasHost.appendChild(renderer.domElement);

  const hemi = new THREE.HemisphereLight(0xffffff, 0x3a4034, 1.05);
  scene.add(hemi);
  const key = new THREE.DirectionalLight(0xffffff, 1.15);
  key.position.set(4, 8, 5);
  scene.add(key);
  const fill = new THREE.DirectionalLight(0xdde6d0, 0.35);
  fill.position.set(-5, 2, -3);
  scene.add(fill);

  const grid = new THREE.GridHelper(10, 20, colors.line, colors.line);
  grid.material.opacity = 0.35;
  grid.material.transparent = true;
  scene.add(grid);

  const controls = new OrbitControls(camera, renderer.domElement);
  controls.enableDamping = true;
  controls.dampingFactor = 0.08;
  controls.screenSpacePanning = true;

  const session = {
    alive: true,
    wrap,
    canvasHost,
    scene,
    camera,
    renderer,
    controls,
    root: null,
    grid,
    raf: 0,
    ro: null,
    wireframe: false,
  };
  active = session;

  const onToolbar = (event) => {
    const btn = event.target.closest("[data-model]");
    if (!btn || !session.root) return;
    const action = btn.getAttribute("data-model");
    if (action === "reset") {
      fitCamera(session.camera, session.controls, session.root);
      return;
    }
    if (action === "wire") {
      session.wireframe = !session.wireframe;
      wireBtn.setAttribute("aria-pressed", session.wireframe ? "true" : "false");
      session.root.traverse((child) => {
        if (!child.isMesh || !child.material) return;
        const mats = Array.isArray(child.material) ? child.material : [child.material];
        for (const mat of mats) {
          if (mat && "wireframe" in mat) mat.wireframe = session.wireframe;
        }
      });
    }
  };
  wrap.addEventListener("click", onToolbar);

  session.ro = new ResizeObserver(() => resize(session));
  session.ro.observe(canvasHost);
  resize(session);

  const tick = () => {
    if (!session.alive) return;
    session.controls.update();
    session.renderer.render(session.scene, session.camera);
    session.raf = requestAnimationFrame(tick);
  };
  session.raf = requestAnimationFrame(tick);

  try {
    const root = await loadModel(url, ext, colors, (text) => setStatus(status, text, false));
    if (!session.alive || active !== session) {
      disposeObject(root);
      return;
    }
    session.root = root;
    scene.add(root);

    // Scale grid to model size.
    const box = new THREE.Box3().setFromObject(root);
    const size = box.getSize(new THREE.Vector3());
    const maxDim = Math.max(size.x, size.y, size.z, 1);
    grid.scale.setScalar(Math.max(maxDim / 5, 0.2));
    grid.position.y = box.min.y;

    fitCamera(camera, controls, root);
    setStatus(status, "", false);
  } catch (err) {
    if (!session.alive || active !== session) return;
    const message = err && err.message ? err.message : "Could not load this 3D model";
    setStatus(status, message, true);
  }
}

/** Render a neutral, camera-fitted thumbnail without changing the active preview. */
export async function renderModelThumbnail({ url, name, size = 0 }) {
  if (!isViewableModel(name)) throw new Error("Unsupported 3D format");
  if (size > 80 * 1024 * 1024) throw new Error("Model exceeds the 80 MB thumbnail limit");
  const colors = { bg: "#20252b", accent: "#c2ccd6", muted: "#8d99a5", line: "#3d4650" };
  let root = null;
  let renderer = null;
  try {
    root = await loadModel(url, extensionOf(name), colors, () => {}, 30000);
    let triangles = 0;
    root.traverse((object) => {
      if (object.isMesh) triangles += (object.geometry.index?.count || object.geometry.getAttribute("position")?.count || 0) / 3;
    });
    if (triangles > 2000000) throw new Error("Model is too detailed for automatic thumbnails");
    const bounds = new THREE.Box3().setFromObject(root);
    if (bounds.isEmpty()) throw new Error("Model has no visible geometry");
    const dimensions = bounds.getSize(new THREE.Vector3());
    const extent = Math.max(dimensions.x, dimensions.y, dimensions.z);
    if (!Number.isFinite(extent) || extent <= 0) throw new Error("Model has no visible geometry");
    const center = bounds.getCenter(new THREE.Vector3());
    root.position.sub(center);
    // Imported STL normals may be all zero, and metallic GLTF materials need an
    // environment map. Use a neutral clay material for predictable thumbnails.
    const importedMaterials = new Set();
    const importedTextures = new Set();
    root.traverse((object) => {
      if (!object.isMesh) return;
      const geometry = object.geometry;
      const normals = geometry.getAttribute("normal");
      let usableNormals = !!normals;
      if (normals) {
        for (let i = 0; i < normals.count; i++) {
          const length = normals.getX(i) ** 2 + normals.getY(i) ** 2 + normals.getZ(i) ** 2;
          if (!Number.isFinite(length) || length < 1e-12) { usableNormals = false; break; }
        }
      }
      if (!usableNormals) geometry.computeVertexNormals();
      for (const material of (Array.isArray(object.material) ? object.material : [object.material])) {
        if (!material) continue;
        importedMaterials.add(material);
        for (const value of Object.values(material)) if (value?.isTexture) importedTextures.add(value);
      }
      object.material = new THREE.MeshStandardMaterial({
        color: colors.accent, metalness: 0, roughness: 0.75, side: THREE.DoubleSide,
      });
    });
    for (const material of importedMaterials) material.dispose();
    for (const texture of importedTextures) texture.dispose();
    // Normalize units so millimeter CAD and tiny meter-based models share the
    // same clipping planes and studio lighting.
    const normalized = new THREE.Group();
    normalized.add(root);
    normalized.scale.setScalar(2 / extent);
    root = normalized;
    const scene = new THREE.Scene();
    scene.background = new THREE.Color(colors.bg);
    scene.add(root);
    scene.add(new THREE.HemisphereLight(0xffffff, 0x526070, 1.5));
    const light = new THREE.DirectionalLight(0xffffff, 2.5);
    light.position.set(4, 8, 5);
    scene.add(light);
    const fill = new THREE.DirectionalLight(0xdde8ff, 1.25);
    fill.position.set(-4, 2, -3);
    scene.add(fill);
    const camera = new THREE.PerspectiveCamera(40, 1, 0.01, 2000);
    const control = { target: new THREE.Vector3(), update() { camera.lookAt(this.target); } };
    fitCamera(camera, control, root);
    renderer = new THREE.WebGLRenderer({ antialias: true, alpha: false, preserveDrawingBuffer: true });
    renderer.outputColorSpace = THREE.SRGBColorSpace;
    renderer.toneMapping = THREE.ACESFilmicToneMapping;
    renderer.toneMappingExposure = 1.15;
    renderer.setSize(480, 480, false);
    renderer.render(scene, camera);
    return await new Promise((resolve, reject) => renderer.domElement.toBlob(
      (blob) => blob ? resolve(blob) : reject(new Error("Could not capture the 3D thumbnail")), "image/png",
    ));
  } finally {
    disposeObject(root);
    if (renderer) { renderer.dispose(); renderer.forceContextLoss?.(); }
  }
}
