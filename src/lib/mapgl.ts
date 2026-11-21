// Draws a map with WebGL2: the values as a float texture, the colour map as a 256x1
// texture, and the range (linear or log) and missing colour in the shader. Changing the
// colour map or range needs no new data.
import type { MapHeader } from "./api";

const VERT = `#version 300 es
in vec2 pos;
out vec2 uv;
void main() {
  uv = vec2((pos.x + 1.0) / 2.0, (1.0 - pos.y) / 2.0);
  gl_Position = vec4(pos, 0.0, 1.0);
}`;

const FRAG = `#version 300 es
precision highp float;
uniform highp sampler2D data;
uniform sampler2D lut;
uniform float lo;
uniform float hi;
uniform bool logscale;
uniform vec3 missing;
in vec2 uv;
out vec4 color;
void main() {
  float v = texture(data, uv).r;
  if (isnan(v) || isinf(v)) { color = vec4(missing, 1.0); return; }
  float t;
  if (logscale) {
    t = v <= 0.0 ? 0.0 : (log(v) - log(lo)) / (log(hi) - log(lo));
  } else {
    t = hi > lo ? (v - lo) / (hi - lo) : 0.5;
  }
  t = clamp(t, 0.0, 1.0);
  color = vec4(texture(lut, vec2((t * 255.0 + 0.5) / 256.0, 0.5)).rgb, 1.0);
}`;

export class MapGL {
  private gl: WebGL2RenderingContext;
  private prog: WebGLProgram;
  private data: WebGLTexture;
  private lut: WebGLTexture;

  constructor(canvas: HTMLCanvasElement) {
    const gl = canvas.getContext("webgl2", { preserveDrawingBuffer: true });
    if (!gl) throw new Error("WebGL2 is not available");
    this.gl = gl;
    this.prog = program(gl, VERT, FRAG);
    gl.useProgram(this.prog);
    const buf = gl.createBuffer();
    gl.bindBuffer(gl.ARRAY_BUFFER, buf);
    gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 1, -1, -1, 1, 1, 1]), gl.STATIC_DRAW);
    const loc = gl.getAttribLocation(this.prog, "pos");
    gl.enableVertexAttribArray(loc);
    gl.vertexAttribPointer(loc, 2, gl.FLOAT, false, 0, 0);
    this.data = texture(gl);
    this.lut = texture(gl);
    gl.uniform1i(gl.getUniformLocation(this.prog, "data"), 0);
    gl.uniform1i(gl.getUniformLocation(this.prog, "lut"), 1);
  }

  /** The colour map: 256 RGB triples. */
  setColormap(lut: number[]) {
    const gl = this.gl;
    gl.activeTexture(gl.TEXTURE1);
    gl.bindTexture(gl.TEXTURE_2D, this.lut);
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGB8, 256, 1, 0, gl.RGB, gl.UNSIGNED_BYTE, new Uint8Array(lut));
  }

  draw(h: MapHeader, values: Float32Array) {
    const gl = this.gl;
    gl.canvas.width = h.w;
    gl.canvas.height = h.h;
    gl.viewport(0, 0, h.w, h.h);
    gl.activeTexture(gl.TEXTURE0);
    gl.bindTexture(gl.TEXTURE_2D, this.data);
    gl.pixelStorei(gl.UNPACK_ALIGNMENT, 4);
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.R32F, h.w, h.h, 0, gl.RED, gl.FLOAT, values);
    const u = (n: string) => gl.getUniformLocation(this.prog, n);
    gl.uniform1f(u("lo"), h.min);
    gl.uniform1f(u("hi"), h.max);
    gl.uniform1i(u("logscale"), h.log ? 1 : 0);
    gl.uniform3f(u("missing"), h.missing[0] / 255, h.missing[1] / 255, h.missing[2] / 255);
    gl.drawArrays(gl.TRIANGLE_STRIP, 0, 4);
  }
}

function texture(gl: WebGL2RenderingContext): WebGLTexture {
  const t = gl.createTexture()!;
  gl.bindTexture(gl.TEXTURE_2D, t);
  for (const p of [gl.TEXTURE_MIN_FILTER, gl.TEXTURE_MAG_FILTER]) gl.texParameteri(gl.TEXTURE_2D, p, gl.NEAREST);
  for (const p of [gl.TEXTURE_WRAP_S, gl.TEXTURE_WRAP_T]) gl.texParameteri(gl.TEXTURE_2D, p, gl.CLAMP_TO_EDGE);
  return t;
}

function program(gl: WebGL2RenderingContext, vs: string, fs: string): WebGLProgram {
  const p = gl.createProgram()!;
  for (const [type, src] of [
    [gl.VERTEX_SHADER, vs],
    [gl.FRAGMENT_SHADER, fs],
  ] as const) {
    const s = gl.createShader(type)!;
    gl.shaderSource(s, src);
    gl.compileShader(s);
    if (!gl.getShaderParameter(s, gl.COMPILE_STATUS)) throw new Error(gl.getShaderInfoLog(s) ?? "shader");
    gl.attachShader(p, s);
  }
  gl.linkProgram(p);
  if (!gl.getProgramParameter(p, gl.LINK_STATUS)) throw new Error(gl.getProgramInfoLog(p) ?? "link");
  return p;
}
