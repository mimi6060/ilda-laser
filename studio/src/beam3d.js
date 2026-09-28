// Laser Studio: 3D beam view (T-275, T-276).
//
// Draws the frame the studio sends (the same points as the 2D preview, after
// calibration and safety) as beams leaving a projector into a hazy room.
// Preview only: this module never talks to the server. The page hands it
// frames with setFrame(); it has no way to arm or send anything.
//
// Raw WebGL2, no library: every pass needs its own shader anyway
// (energy-conserving alpha, haze attenuation, noise, sheets, halo), and a
// dependency-free file keeps the studio working offline.
//
// World: metres, y up. The room is a box x ∈ [-w/2, w/2], y ∈ [0, h],
// z ∈ [0, d]; the stage is at the far end (z = d), the audience towards
// z = 0. The default projector sits at the stage edge, 3 m high, facing
// the audience (-z).

export const DEFAULT_ROOM = { w: 10, d: 15, h: 5 };
export const DEFAULT_PROJECTOR = { pos: [0, 3, 11], yaw: 0, pitch: 0, roll: 0, scanDeg: 40 };
export const DEFAULT_RENDER = { exposure: 1.0, sheets: true, haze: 0.5, hazeNoise: false, roomLight: 0.1, spots: true, bloom: true, quality: 'auto' };
export const QUALITIES = ['low', 'medium', 'high'];

const DEG = Math.PI / 180;
// Gains picked so a static beam saturates and a 200-point sweep is a faint
// but readable plane at exposure 1 and haze 50 %.
const BEAM_GAIN = 6, SHEET_GAIN = 5, SPOT_GAIN = 3;
const BEAM_WIDTH_PX = 2;
const BEAM_STRIDE = 7;   // hit x,y,z, r,g,b, weight
const SHEET_STRIDE = 7;  // pos x,y,z, r,g,b, weight (3 vertices per triangle)

// ---------- geometry (pure, unit-tested from the e2e suite) ----------

/** The projector's right / up / forward unit vectors in world space. */
export function projectorBasis(p) {
  const cy = Math.cos((p.yaw || 0) * DEG), sy = Math.sin((p.yaw || 0) * DEG);
  const cp = Math.cos((p.pitch || 0) * DEG), sp = Math.sin((p.pitch || 0) * DEG);
  const cr = Math.cos((p.roll || 0) * DEG), sr = Math.sin((p.roll || 0) * DEG);
  // yaw 0, pitch 0: forward -z, right +x, up +y.
  const f = [sy * cp, sp, -cy * cp];
  const r0 = [cy, 0, sy];
  const u0 = cross(r0, f);
  const r = [cr * r0[0] + sr * u0[0], cr * r0[1] + sr * u0[1], cr * r0[2] + sr * u0[2]];
  const u = [-sr * r0[0] + cr * u0[0], -sr * r0[1] + cr * u0[1], -sr * r0[2] + cr * u0[2]];
  return { r, u, f };
}

/**
 * Output point (x, y) in -1..1 → unit direction in world space:
 * yaw = x·θ/2, pitch = y·θ/2 in the projector's frame (θ = scan angle).
 */
export function pointToDir(x, y, projector, out = [0, 0, 0], basis = projectorBasis(projector)) {
  const half = (projector.scanDeg ?? 40) * DEG / 2;
  const a = x * half, b = y * half;
  const lx = Math.sin(a) * Math.cos(b), ly = Math.sin(b), lz = Math.cos(a) * Math.cos(b);
  const { r, u, f } = basis;
  out[0] = r[0] * lx + u[0] * ly + f[0] * lz;
  out[1] = r[1] * lx + u[1] * ly + f[1] * lz;
  out[2] = r[2] * lx + u[2] * ly + f[2] * lz;
  return out;
}

/**
 * Distance along the ray (origin inside the room) to where it leaves the
 * box, and which face it hits: 0/1 = left/right wall, 2/3 = floor/ceiling,
 * 4/5 = audience-end / stage-end wall.
 */
export function rayBox(o, d, room) {
  const lo = [-room.w / 2, 0, 0], hi = [room.w / 2, room.h, room.d];
  let t = Infinity, face = -1;
  for (let i = 0; i < 3; i++) {
    if (d[i] > 1e-9) { const ti = (hi[i] - o[i]) / d[i]; if (ti < t) { t = ti; face = i * 2 + 1; } }
    else if (d[i] < -1e-9) { const ti = (lo[i] - o[i]) / d[i]; if (ti < t) { t = ti; face = i * 2; } }
  }
  return { t: Math.max(0, t), face };
}

/** Energy conservation: each sample gets exposure / N (N = samples in the frame). */
export function beamAlpha(n, exposure = 1) {
  return n > 0 ? exposure / n : 0;
}

export const isLit = p => p[2] + p[3] + p[4] > 0;

/**
 * Fills `geo.beam` (one instance per lit point) and `geo.sheet` (one thin
 * triangle projector → hit i → hit i+1 per pair of consecutive lit points)
 * from a frame. Buffers are reused and only grow. Returns the counts.
 */
export function buildGeometry(points, projector, room, geo, withSheets = true) {
  const n = points.length;
  if (!geo.beam || geo.beam.length < n * BEAM_STRIDE) geo.beam = new Float32Array(Math.max(1024, n * 2) * BEAM_STRIDE);
  if (withSheets && (!geo.sheet || geo.sheet.length < n * 3 * SHEET_STRIDE)) geo.sheet = new Float32Array(Math.max(1024, n * 2) * 3 * SHEET_STRIDE);
  const basis = projectorBasis(projector), o = projector.pos, w = beamAlpha(n);
  const dir = [0, 0, 0];
  let beams = 0, sheets = 0, prev = -1; // prev = beam index of the previous point if it was lit
  for (let i = 0; i < n; i++) {
    const p = points[i];
    if (!isLit(p)) { prev = -1; continue; }
    pointToDir(p[0], p[1], projector, dir, basis);
    const t = rayBox(o, dir, room).t;
    const b = geo.beam, k = beams * BEAM_STRIDE;
    b[k] = o[0] + dir[0] * t; b[k + 1] = o[1] + dir[1] * t; b[k + 2] = o[2] + dir[2] * t;
    b[k + 3] = p[2]; b[k + 4] = p[3]; b[k + 5] = p[4]; b[k + 6] = w;
    if (withSheets && prev >= 0) {
      const s = geo.sheet, j = prev * BEAM_STRIDE;
      const m = sheets * 3 * SHEET_STRIDE;
      s[m] = o[0]; s[m + 1] = o[1]; s[m + 2] = o[2];
      s[m + 7] = b[j]; s[m + 8] = b[j + 1]; s[m + 9] = b[j + 2];
      s[m + 14] = b[k]; s[m + 15] = b[k + 1]; s[m + 16] = b[k + 2];
      for (let v = m + 3; v < m + 3 * SHEET_STRIDE; v += SHEET_STRIDE) { s[v] = p[2]; s[v + 1] = p[3]; s[v + 2] = p[4]; s[v + 3] = w; }
      sheets++;
    }
    prev = beams++;
  }
  return { beams, sheets };
}

/**
 * Automatic quality: drops one level when frames take longer than 20 ms
 * for 2 s in a row. Never climbs back up by itself (no oscillation).
 */
export class AutoQuality {
  constructor(level = 2, limitMs = 20, holdMs = 2000) { this.level = level; this.limitMs = limitMs; this.holdMs = holdMs; this.slowSince = null; }
  sample(frameMs, nowMs) {
    if (frameMs <= this.limitMs) { this.slowSince = null; return this.level; }
    if (this.slowSince === null) this.slowSince = nowMs;
    else if (nowMs - this.slowSince >= this.holdMs && this.level > 0) { this.level--; this.slowSince = null; }
    return this.level;
  }
  get name() { return QUALITIES[this.level]; }
}

/** What a quality level turns on (the user's own switches can only turn things off). */
export function qualityFeatures(level, render) {
  return {
    sheets: render.sheets && level >= 1,
    bloomLevels: render.bloom ? level : 0, // low 0, medium 1, high 2
    noise: render.hazeNoise && level >= 2,
    scale: [0.5, 0.75, 1][level],
  };
}

// ---------- small matrix helpers (column-major, as WebGL wants) ----------

function cross(a, b) { return [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]; }
function norm(a) { const l = Math.hypot(a[0], a[1], a[2]) || 1; return [a[0] / l, a[1] / l, a[2] / l]; }
function perspective(fovy, aspect, near, far) {
  const f = 1 / Math.tan(fovy / 2), nf = 1 / (near - far);
  return [f / aspect, 0, 0, 0, 0, f, 0, 0, 0, 0, (far + near) * nf, -1, 0, 0, 2 * far * near * nf, 0];
}
function lookAt(eye, target, up) {
  const z = norm([eye[0] - target[0], eye[1] - target[1], eye[2] - target[2]]);
  const x = norm(cross(up, z)), y = cross(z, x);
  return [x[0], y[0], z[0], 0, x[1], y[1], z[1], 0, x[2], y[2], z[2], 0,
    -(x[0] * eye[0] + x[1] * eye[1] + x[2] * eye[2]), -(y[0] * eye[0] + y[1] * eye[1] + y[2] * eye[2]), -(z[0] * eye[0] + z[1] * eye[1] + z[2] * eye[2]), 1];
}
function mul(a, b) {
  const o = new Array(16);
  for (let c = 0; c < 4; c++) for (let r = 0; r < 4; r++) {
    o[c * 4 + r] = a[r] * b[c * 4] + a[4 + r] * b[c * 4 + 1] + a[8 + r] * b[c * 4 + 2] + a[12 + r] * b[c * 4 + 3];
  }
  return o;
}

// ---------- shaders ----------

const VS_BEAM = `#version 300 es
layout(location=0) in vec2 aCorner;   // x: 0 at the projector, 1 at the hit; y: side -1..1
layout(location=1) in vec3 iHit;
layout(location=2) in vec4 iColor;    // rgb, weight 1/N
uniform mat4 uViewProj; uniform vec3 uOrigin; uniform vec2 uViewport; uniform float uWidth;
out vec3 vWorld; out vec4 vColor; out float vSide;
void main() {
  vec4 c0 = uViewProj * vec4(uOrigin, 1.0), c1 = uViewProj * vec4(iHit, 1.0);
  const float E = 0.05;  // keep the segment in front of the camera
  vColor = iColor; vSide = aCorner.y;
  if (c0.w < E && c1.w < E) { gl_Position = vec4(2.0, 2.0, 2.0, 1.0); vWorld = uOrigin; vColor = vec4(0.0); return; }
  float t0 = c0.w < E ? (E - c0.w) / (c1.w - c0.w) : 0.0;
  float t1 = c1.w < E ? (E - c0.w) / (c1.w - c0.w) : 1.0;
  vec4 a = mix(c0, c1, t0), b = mix(c0, c1, t1);
  float t = mix(t0, t1, aCorner.x);
  vec4 c = mix(c0, c1, t);
  vec2 dir = (b.xy / b.w - a.xy / a.w) * uViewport;
  float len = length(dir);
  vec2 n = len > 1e-4 ? vec2(-dir.y, dir.x) / len : vec2(0.0, 1.0);
  c.xy += n * aCorner.y * uWidth / uViewport * c.w;
  gl_Position = c;
  vWorld = mix(uOrigin, iHit, t);
}`;

const VS_SHEET = `#version 300 es
layout(location=0) in vec3 aPos; layout(location=1) in vec4 aColor;
uniform mat4 uViewProj;
out vec3 vWorld; out vec4 vColor; out float vSide;
void main() { gl_Position = uViewProj * vec4(aPos, 1.0); vWorld = aPos; vColor = aColor; vSide = 0.0; }`;

// Shared by beams and sheets: energy weight × haze × exp(-σ·d) × clouds.
const FS_HAZE = `#version 300 es
precision highp float;
in vec3 vWorld; in vec4 vColor; in float vSide;
uniform vec3 uOrigin; uniform float uGain, uSigma, uNoise, uTime;
out vec4 o;
float hash(vec3 p) { p = fract(p * 0.3183099 + 0.1); p *= 17.0; return fract(p.x * p.y * p.z * (p.x + p.y + p.z)); }
float vnoise(vec3 x) {
  vec3 i = floor(x), f = fract(x); f = f * f * (3.0 - 2.0 * f);
  return mix(mix(mix(hash(i), hash(i + vec3(1,0,0)), f.x), mix(hash(i + vec3(0,1,0)), hash(i + vec3(1,1,0)), f.x), f.y),
             mix(mix(hash(i + vec3(0,0,1)), hash(i + vec3(1,0,1)), f.x), mix(hash(i + vec3(0,1,1)), hash(i + vec3(1,1,1)), f.x), f.y), f.z);
}
void main() {
  float d = distance(vWorld, uOrigin);
  float k = uGain * vColor.a * exp(-uSigma * d) * (1.0 - vSide * vSide);
  if (uNoise > 0.0) {
    vec3 p = vWorld * 0.35 + vec3(0.07, 0.02, 0.05) * uTime;
    float n = vnoise(p) * 0.65 + vnoise(p * 2.3) * 0.35;
    k *= mix(1.0, 0.2 + 1.6 * n, uNoise);
  }
  o = vec4(vColor.rgb * k, 1.0);
}`;

const VS_SPOT = `#version 300 es
layout(location=1) in vec3 iHit; layout(location=2) in vec4 iColor;
uniform mat4 uViewProj; uniform float uSize;
out vec4 vColor;
void main() { gl_Position = uViewProj * vec4(iHit, 1.0); gl_PointSize = uSize; vColor = iColor; }`;

const FS_SPOT = `#version 300 es
precision highp float;
in vec4 vColor; uniform float uGain; out vec4 o;
void main() {
  vec2 q = gl_PointCoord * 2.0 - 1.0; float r2 = dot(q, q);
  if (r2 > 1.0) discard;
  o = vec4(vColor.rgb * vColor.a * uGain * (1.0 - r2) * (1.0 - r2), 1.0);
}`;

const VS_ROOM = `#version 300 es
layout(location=0) in vec3 aPos; layout(location=1) in vec4 aColor;  // rgb, minimum level
uniform mat4 uViewProj; uniform float uLight;
out vec3 vC;
void main() { gl_Position = uViewProj * vec4(aPos, 1.0); vC = aColor.rgb * max(uLight, aColor.a); }`;

const FS_ROOM = `#version 300 es
precision highp float; in vec3 vC; out vec4 o; void main() { o = vec4(vC, 1.0); }`;

const VS_QUAD = `#version 300 es
out vec2 vUv;
void main() { vec2 p = vec2((gl_VertexID << 1) & 2, gl_VertexID & 2); vUv = p; gl_Position = vec4(p * 2.0 - 1.0, 0.0, 1.0); }`;

// 9-tap Gaussian in 5 bilinear fetches; reading a bigger texture also downsamples.
const FS_BLUR = `#version 300 es
precision highp float; in vec2 vUv; uniform sampler2D uTex; uniform vec2 uStep; out vec4 o;
void main() {
  vec3 c = texture(uTex, vUv).rgb * 0.2270270;
  c += (texture(uTex, vUv + uStep * 1.3846154).rgb + texture(uTex, vUv - uStep * 1.3846154).rgb) * 0.3162162;
  c += (texture(uTex, vUv + uStep * 3.2307692).rgb + texture(uTex, vUv - uStep * 3.2307692).rgb) * 0.0702703;
  o = vec4(c, 1.0);
}`;

const FS_COMPOSITE = `#version 300 es
precision highp float; in vec2 vUv;
uniform sampler2D uScene, uB1, uB2; uniform float uBloom1, uBloom2; out vec4 o;
void main() {
  vec3 c = texture(uScene, vUv).rgb + uBloom1 * texture(uB1, vUv).rgb + uBloom2 * texture(uB2, vUv).rgb;
  c = 1.0 - exp(-c);               // soft saturation: stacked beams stay white-hot, not clipped flat
  o = vec4(pow(c, vec3(1.0 / 2.2)), 1.0);
}`;

// ---------- the view ----------

export class BeamView {
  /** WebGL2 available? (A throw-away canvas, so the real one keeps its context type free.) */
  static supported() {
    try { return !!document.createElement('canvas').getContext('webgl2'); } catch { return false; }
  }

  constructor(canvas, { room = DEFAULT_ROOM, projector = DEFAULT_PROJECTOR, render = DEFAULT_RENDER } = {}) {
    const gl = canvas.getContext('webgl2', { antialias: false, alpha: false, depth: false, premultipliedAlpha: false });
    if (!gl) throw new Error('WebGL2 unavailable');
    this.canvas = canvas; this.gl = gl;
    this.room = { ...room }; this.projector = { ...projector, pos: [...projector.pos] };
    this.render = { ...render };
    this.auto = new AutoQuality();
    this.geo = {}; this.counts = { beams: 0, sheets: 0 };
    this.points = []; this.dirty = false;
    this.running = false; this.raf = 0; this.lastT = 0;
    this.stats = { frameMs: 0, drawMs: 0, buildMs: 0 };
    // Float targets so thousands of faint samples (1/N each) add up instead
    // of rounding to zero in 8 bits.
    this.hdr = !!gl.getExtension('EXT_color_buffer_float');
    this.progs = {
      beam: this.program(VS_BEAM, FS_HAZE), sheet: this.program(VS_SHEET, FS_HAZE),
      spot: this.program(VS_SPOT, FS_SPOT), room: this.program(VS_ROOM, FS_ROOM),
      blur: this.program(VS_QUAD, FS_BLUR), comp: this.program(VS_QUAD, FS_COMPOSITE),
    };
    this.buildBuffers();
    this.targets = null;
    this.recenter();
    this.bindControls();
  }

  program(vs, fs) {
    const gl = this.gl, p = gl.createProgram();
    for (const [type, src] of [[gl.VERTEX_SHADER, vs], [gl.FRAGMENT_SHADER, fs]]) {
      const s = gl.createShader(type);
      gl.shaderSource(s, src); gl.compileShader(s);
      if (!gl.getShaderParameter(s, gl.COMPILE_STATUS)) throw new Error('shader: ' + gl.getShaderInfoLog(s));
      gl.attachShader(p, s);
    }
    gl.linkProgram(p);
    if (!gl.getProgramParameter(p, gl.LINK_STATUS)) throw new Error('program: ' + gl.getProgramInfoLog(p));
    const u = {}, n = gl.getProgramParameter(p, gl.ACTIVE_UNIFORMS);
    for (let i = 0; i < n; i++) { const name = gl.getActiveUniform(p, i).name; u[name] = gl.getUniformLocation(p, name); }
    return { p, u };
  }

  buildBuffers() {
    const gl = this.gl;
    // Beams: a 2-triangle strip per instance, corners (t, side).
    this.cornerBuf = gl.createBuffer();
    gl.bindBuffer(gl.ARRAY_BUFFER, this.cornerBuf);
    gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([0, -1, 1, -1, 0, 1, 1, 1]), gl.STATIC_DRAW);
    this.beamBuf = gl.createBuffer(); this.sheetBuf = gl.createBuffer(); this.roomBuf = gl.createBuffer();

    const attrib = (loc, size, stride, offset, divisor) => {
      gl.enableVertexAttribArray(loc);
      gl.vertexAttribPointer(loc, size, gl.FLOAT, false, stride * 4, offset * 4);
      gl.vertexAttribDivisor(loc, divisor);
    };
    this.beamVao = gl.createVertexArray(); gl.bindVertexArray(this.beamVao);
    gl.bindBuffer(gl.ARRAY_BUFFER, this.cornerBuf); attrib(0, 2, 2, 0, 0);
    gl.bindBuffer(gl.ARRAY_BUFFER, this.beamBuf); attrib(1, 3, BEAM_STRIDE, 0, 1); attrib(2, 4, BEAM_STRIDE, 3, 1);
    this.spotVao = gl.createVertexArray(); gl.bindVertexArray(this.spotVao);
    gl.bindBuffer(gl.ARRAY_BUFFER, this.beamBuf); attrib(1, 3, BEAM_STRIDE, 0, 0); attrib(2, 4, BEAM_STRIDE, 3, 0);
    this.sheetVao = gl.createVertexArray(); gl.bindVertexArray(this.sheetVao);
    gl.bindBuffer(gl.ARRAY_BUFFER, this.sheetBuf); attrib(0, 3, SHEET_STRIDE, 0, 0); attrib(1, 4, SHEET_STRIDE, 3, 0);
    this.roomVao = gl.createVertexArray(); gl.bindVertexArray(this.roomVao);
    gl.bindBuffer(gl.ARRAY_BUFFER, this.roomBuf); attrib(0, 3, 7, 0, 0); attrib(1, 4, 7, 3, 0);
    gl.bindVertexArray(null);
    this.quadVao = gl.createVertexArray();
    this.buildRoom();
  }

  /** Floor (filled), floor grid, box edges and a projector marker. */
  buildRoom() {
    const { w, d, h } = this.room, x0 = -w / 2, x1 = w / 2, v = [];
    const vert = (p, c, min) => v.push(p[0], p[1], p[2], c, c, c * 1.1, min);
    // floor fill (triangles)
    const floor = [[x0, 0, 0], [x1, 0, 0], [x1, 0, d], [x0, 0, 0], [x1, 0, d], [x0, 0, d]];
    for (const p of floor) vert(p, 0.15, 0);
    this.roomTris = 6;
    const line = (a, b, c, min = 0) => { vert(a, c, min); vert(b, c, min); };
    for (let x = Math.ceil(x0); x <= x1; x++) line([x, 0, 0], [x, 0, d], 0.35);
    for (let z = 0; z <= d; z++) line([x0, 0, z], [x1, 0, z], 0.35);
    const c = [[x0, 0, 0], [x1, 0, 0], [x1, 0, d], [x0, 0, d], [x0, h, 0], [x1, h, 0], [x1, h, d], [x0, h, d]];
    for (const [a, b] of [[0, 1], [1, 2], [2, 3], [3, 0], [4, 5], [5, 6], [6, 7], [7, 4], [0, 4], [1, 5], [2, 6], [3, 7]]) line(c[a], c[b], 0.9, 0.03);
    // projector: a small box, always visible
    const [px, py, pz] = this.projector.pos, s = 0.15;
    const q = [[-s, -s, -s], [s, -s, -s], [s, s, -s], [-s, s, -s], [-s, -s, s], [s, -s, s], [s, s, s], [-s, s, s]].map(o => [px + o[0], py + o[1], pz + o[2]]);
    for (const [a, b] of [[0, 1], [1, 2], [2, 3], [3, 0], [4, 5], [5, 6], [6, 7], [7, 4], [0, 4], [1, 5], [2, 6], [3, 7]]) line(q[a], q[b], 0.6, 0.6);
    this.roomLines = v.length / 7 - this.roomTris;
    const gl = this.gl;
    gl.bindBuffer(gl.ARRAY_BUFFER, this.roomBuf);
    gl.bufferData(gl.ARRAY_BUFFER, new Float32Array(v), gl.STATIC_DRAW);
  }

  // ----- data in -----

  /** A new frame: [x, y, r, g, b] points, as the frame endpoint sends them. */
  setFrame(points) { this.points = points; this.dirty = true; }

  setRender(patch) {
    const sheetsBefore = this.features().sheets;
    Object.assign(this.render, patch);
    if (patch.quality !== undefined && patch.quality === 'auto') this.auto = new AutoQuality();
    if (this.features().sheets !== sheetsBefore) this.dirty = true;
  }

  qualityLevel() { return this.render.quality === 'auto' ? this.auto.level : Math.max(0, QUALITIES.indexOf(this.render.quality)); }
  features() { return qualityFeatures(this.qualityLevel(), this.render); }

  upload() {
    const t0 = performance.now(), gl = this.gl, f = this.features();
    this.counts = buildGeometry(this.points, this.projector, this.room, this.geo, f.sheets);
    if (!f.sheets) this.counts.sheets = 0;
    gl.bindBuffer(gl.ARRAY_BUFFER, this.beamBuf);
    gl.bufferData(gl.ARRAY_BUFFER, this.geo.beam.subarray(0, Math.max(1, this.counts.beams) * BEAM_STRIDE), gl.DYNAMIC_DRAW);
    if (this.counts.sheets) {
      gl.bindBuffer(gl.ARRAY_BUFFER, this.sheetBuf);
      gl.bufferData(gl.ARRAY_BUFFER, this.geo.sheet.subarray(0, this.counts.sheets * 3 * SHEET_STRIDE), gl.DYNAMIC_DRAW);
    }
    this.dirty = false;
    this.stats.buildMs = performance.now() - t0;
  }

  // ----- camera -----

  recenter() {
    const { w, d, h } = this.room;
    // From the audience side, a little to the right and above head height.
    this.cam = { target: [0, h * 0.4, d * 0.55], az: Math.PI - 0.4, el: 0.3, dist: Math.max(w, d) * 1.75 };
  }
  setCamera(c) { Object.assign(this.cam, c); }

  viewProj(aspect) {
    const { target: t, az, el, dist } = this.cam;
    const eye = [t[0] + dist * Math.cos(el) * Math.sin(az), t[1] + dist * Math.sin(el), t[2] + dist * Math.cos(el) * Math.cos(az)];
    return mul(perspective(50 * DEG, aspect, 0.1, 500), lookAt(eye, t, [0, 1, 0]));
  }

  bindControls() {
    const cv = this.canvas, ptrs = new Map();
    let pinch = 0;
    cv.style.touchAction = 'none';
    cv.addEventListener('pointerdown', e => { cv.setPointerCapture(e.pointerId); ptrs.set(e.pointerId, [e.clientX, e.clientY]); pinch = 0; });
    const up = e => { ptrs.delete(e.pointerId); pinch = 0; };
    cv.addEventListener('pointerup', up); cv.addEventListener('pointercancel', up);
    cv.addEventListener('pointermove', e => {
      const last = ptrs.get(e.pointerId);
      if (!last) return;
      const dx = e.clientX - last[0], dy = e.clientY - last[1];
      ptrs.set(e.pointerId, [e.clientX, e.clientY]);
      if (ptrs.size >= 2) {
        const [a, b] = [...ptrs.values()], dd = Math.hypot(a[0] - b[0], a[1] - b[1]);
        if (pinch) this.cam.dist = clamp(this.cam.dist * pinch / dd, 2, 120);
        pinch = dd;
        this.pan(dx / 2, dy / 2);
      } else if (e.shiftKey || e.buttons === 4 || e.buttons === 2) {
        this.pan(dx, dy);
      } else {
        this.cam.az -= dx * 0.006;
        this.cam.el = clamp(this.cam.el + dy * 0.006, -0.2, 1.5);
      }
    });
    cv.addEventListener('wheel', e => { e.preventDefault(); this.cam.dist = clamp(this.cam.dist * Math.exp(e.deltaY * 0.001), 2, 120); }, { passive: false });
    cv.addEventListener('dblclick', () => this.recenter());
    cv.addEventListener('contextmenu', e => e.preventDefault());
  }

  pan(dx, dy) {
    const { az, el, dist } = this.cam, k = dist * 0.0015;
    const right = [Math.cos(az), 0, -Math.sin(az)];
    const up = [-Math.sin(el) * Math.sin(az), Math.cos(el), -Math.sin(el) * Math.cos(az)];
    for (let i = 0; i < 3; i++) this.cam.target[i] += (-right[i] * dx + up[i] * dy) * k;
  }

  // ----- render loop -----

  start() {
    if (this.running) return;
    this.running = true; this.lastT = 0;
    const tick = now => {
      if (!this.running) return;
      this.raf = requestAnimationFrame(tick);
      if (this.lastT) {
        this.stats.frameMs = now - this.lastT;
        if (this.render.quality === 'auto') this.auto.sample(this.stats.frameMs, now);
      }
      this.lastT = now;
      this.draw(now / 1000);
    };
    this.raf = requestAnimationFrame(tick);
  }

  stop() { this.running = false; cancelAnimationFrame(this.raf); }

  target(w, h) {
    const gl = this.gl, tex = gl.createTexture(), fb = gl.createFramebuffer();
    gl.bindTexture(gl.TEXTURE_2D, tex);
    if (this.hdr) gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA16F, w, h, 0, gl.RGBA, gl.HALF_FLOAT, null);
    else gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA8, w, h, 0, gl.RGBA, gl.UNSIGNED_BYTE, null);
    for (const [k, v] of [[gl.TEXTURE_MIN_FILTER, gl.LINEAR], [gl.TEXTURE_MAG_FILTER, gl.LINEAR], [gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE], [gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE]]) gl.texParameteri(gl.TEXTURE_2D, k, v);
    gl.bindFramebuffer(gl.FRAMEBUFFER, fb);
    gl.framebufferTexture2D(gl.FRAMEBUFFER, gl.COLOR_ATTACHMENT0, gl.TEXTURE_2D, tex, 0);
    return { tex, fb, w, h };
  }

  ensureTargets(scale) {
    const cv = this.canvas, dpr = Math.min(window.devicePixelRatio || 1, 2);
    const W = Math.max(16, Math.round((cv.clientWidth || cv.width) * dpr)), H = Math.max(16, Math.round((cv.clientHeight || cv.height) * dpr));
    if (cv.width !== W || cv.height !== H) { cv.width = W; cv.height = H; }
    const sw = Math.max(8, Math.round(W * scale)), sh = Math.max(8, Math.round(H * scale));
    const t = this.targets;
    if (t && t.scene.w === sw && t.scene.h === sh) return t;
    if (t) for (const x of Object.values(t)) { this.gl.deleteTexture(x.tex); this.gl.deleteFramebuffer(x.fb); }
    const hw = Math.max(4, sw >> 1), hh = Math.max(4, sh >> 1);
    this.targets = {
      scene: this.target(sw, sh), a1: this.target(hw, hh), b1: this.target(hw, hh),
      a2: this.target(Math.max(2, hw >> 1), Math.max(2, hh >> 1)), b2: this.target(Math.max(2, hw >> 1), Math.max(2, hh >> 1)),
    };
    return this.targets;
  }

  /** Draws one image. Also usable on its own (tests, stills). */
  draw(timeS = performance.now() / 1000) {
    const t0 = performance.now(), gl = this.gl, f = this.features(), r = this.render;
    if (this.dirty) this.upload();
    const T = this.ensureTargets(f.scale), S = T.scene;
    const vp = this.viewProj(S.w / S.h);
    this.lastViewProj = vp;

    gl.bindFramebuffer(gl.FRAMEBUFFER, S.fb);
    gl.viewport(0, 0, S.w, S.h);
    gl.clearColor(0, 0, 0, 1); gl.clear(gl.COLOR_BUFFER_BIT);
    gl.enable(gl.BLEND); gl.blendFunc(gl.ONE, gl.ONE);

    // room
    let P = this.progs.room;
    gl.useProgram(P.p);
    gl.uniformMatrix4fv(P.u.uViewProj, false, vp); gl.uniform1f(P.u.uLight, r.roomLight);
    gl.bindVertexArray(this.roomVao);
    gl.drawArrays(gl.TRIANGLES, 0, this.roomTris);
    gl.drawArrays(gl.LINES, this.roomTris, this.roomLines);

    const hazeGain = r.exposure * r.haze, sigma = 0.04 * r.haze, noise = f.noise ? 1 : 0;
    const setHaze = (P, gain) => {
      gl.uniformMatrix4fv(P.u.uViewProj, false, vp); gl.uniform3fv(P.u.uOrigin, this.projector.pos);
      gl.uniform1f(P.u.uGain, gain); gl.uniform1f(P.u.uSigma, sigma); gl.uniform1f(P.u.uNoise, noise); gl.uniform1f(P.u.uTime, timeS);
    };
    if (hazeGain > 0 && this.counts.sheets) {
      P = this.progs.sheet; gl.useProgram(P.p); setHaze(P, hazeGain * SHEET_GAIN);
      gl.bindVertexArray(this.sheetVao);
      gl.drawArrays(gl.TRIANGLES, 0, this.counts.sheets * 3);
    }
    if (hazeGain > 0 && this.counts.beams) {
      P = this.progs.beam; gl.useProgram(P.p); setHaze(P, hazeGain * BEAM_GAIN);
      gl.uniform2f(P.u.uViewport, S.w, S.h); gl.uniform1f(P.u.uWidth, BEAM_WIDTH_PX * Math.max(0.75, f.scale));
      gl.bindVertexArray(this.beamVao);
      gl.drawArraysInstanced(gl.TRIANGLE_STRIP, 0, 4, this.counts.beams);
    }
    if (r.spots && this.counts.beams) {
      P = this.progs.spot; gl.useProgram(P.p);
      gl.uniformMatrix4fv(P.u.uViewProj, false, vp); gl.uniform1f(P.u.uGain, r.exposure * SPOT_GAIN);
      gl.uniform1f(P.u.uSize, Math.max(3, 7 * f.scale * (S.h / 800)));
      gl.bindVertexArray(this.spotVao);
      gl.drawArrays(gl.POINTS, 0, this.counts.beams);
    }
    gl.disable(gl.BLEND);

    // halo: blur the scene at half then quarter resolution
    P = this.progs.blur; gl.useProgram(P.p); gl.bindVertexArray(this.quadVao);
    const pass = (src, dst, dx, dy) => {
      gl.bindFramebuffer(gl.FRAMEBUFFER, dst.fb); gl.viewport(0, 0, dst.w, dst.h);
      gl.activeTexture(gl.TEXTURE0); gl.bindTexture(gl.TEXTURE_2D, src.tex); gl.uniform1i(P.u.uTex, 0);
      gl.uniform2f(P.u.uStep, dx / src.w, dy / src.h);
      gl.drawArrays(gl.TRIANGLES, 0, 3);
    };
    if (f.bloomLevels >= 1) { pass(S, T.a1, 1, 0); pass(T.a1, T.b1, 0, 1); }
    if (f.bloomLevels >= 2) { pass(T.b1, T.a2, 1, 0); pass(T.a2, T.b2, 0, 1); }

    P = this.progs.comp; gl.useProgram(P.p);
    gl.bindFramebuffer(gl.FRAMEBUFFER, null); gl.viewport(0, 0, this.canvas.width, this.canvas.height);
    [S, T.b1, T.b2].forEach((x, i) => { gl.activeTexture(gl.TEXTURE0 + i); gl.bindTexture(gl.TEXTURE_2D, x.tex); });
    gl.uniform1i(P.u.uScene, 0); gl.uniform1i(P.u.uB1, 1); gl.uniform1i(P.u.uB2, 2);
    gl.uniform1f(P.u.uBloom1, f.bloomLevels >= 1 ? 0.8 : 0); gl.uniform1f(P.u.uBloom2, f.bloomLevels >= 2 ? 0.6 : 0);
    gl.drawArrays(gl.TRIANGLES, 0, 3);
    gl.bindVertexArray(null);
    this.stats.drawMs = performance.now() - t0;
  }

  /** Canvas pixel (origin bottom-left) where a world point lands, or null if behind the camera. */
  project(p) {
    const m = this.lastViewProj || this.viewProj(this.canvas.width / this.canvas.height);
    const x = m[0] * p[0] + m[4] * p[1] + m[8] * p[2] + m[12], y = m[1] * p[0] + m[5] * p[1] + m[9] * p[2] + m[13];
    const w = m[3] * p[0] + m[7] * p[1] + m[11] * p[2] + m[15];
    if (w <= 0) return null;
    return [Math.round((x / w + 1) / 2 * this.canvas.width), Math.round((y / w + 1) / 2 * this.canvas.height)];
  }

  /** Draws, then returns the brightest r+g+b (0..765) around each world point. For tests. */
  probe(worldPoints, radius = 2) {
    this.draw();
    const gl = this.gl, size = radius * 2 + 1, buf = new Uint8Array(size * size * 4);
    return worldPoints.map(p => {
      const px = this.project(p);
      if (!px) return 0;
      gl.readPixels(px[0] - radius, px[1] - radius, size, size, gl.RGBA, gl.UNSIGNED_BYTE, buf);
      let best = 0;
      for (let i = 0; i < buf.length; i += 4) best = Math.max(best, buf[i] + buf[i + 1] + buf[i + 2]);
      return best;
    });
  }
}

function clamp(v, lo, hi) { return Math.min(hi, Math.max(lo, v)); }
