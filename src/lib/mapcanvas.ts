// Draws a map with a plain 2D canvas, for webviews without WebGL2 (some Linux drivers
// and virtual machines): the same colours as the shader in mapgl.ts, computed here.
import type { MapHeader } from "./api";

export class MapCanvas {
  private ctx: CanvasRenderingContext2D;
  private lut: Uint8Array = new Uint8Array(768);

  constructor(canvas: HTMLCanvasElement) {
    const ctx = canvas.getContext("2d");
    if (!ctx) throw new Error("no 2D canvas");
    this.ctx = ctx;
  }

  /** The colour map: 256 RGB triples. */
  setColormap(lut: number[]) {
    this.lut = new Uint8Array(lut);
  }

  draw(h: MapHeader, values: Float32Array) {
    const canvas = this.ctx.canvas;
    canvas.width = h.w;
    canvas.height = h.h;
    const img = this.ctx.createImageData(h.w, h.h);
    const px = img.data;
    const [llo, lhi] = [Math.log(h.min), Math.log(h.max)];
    for (let i = 0; i < values.length; i++) {
      const v = values[i];
      let r, g, b;
      if (!Number.isFinite(v)) {
        [r, g, b] = h.missing;
      } else {
        let t = h.log
          ? v <= 0 ? 0 : (Math.log(v) - llo) / (lhi - llo)
          : h.max > h.min ? (v - h.min) / (h.max - h.min) : 0.5;
        t = Math.min(1, Math.max(0, t));
        const k = 3 * Math.round(t * 255);
        [r, g, b] = [this.lut[k], this.lut[k + 1], this.lut[k + 2]];
      }
      px[4 * i] = r;
      px[4 * i + 1] = g;
      px[4 * i + 2] = b;
      px[4 * i + 3] = 255;
    }
    this.ctx.putImageData(img, 0, 0);
  }
}
