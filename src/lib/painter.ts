// What draws the map: WebGL2 where the webview has it, else a 2D canvas.
import type { MapHeader } from "./api";
import { MapCanvas } from "./mapcanvas";
import { MapGL } from "./mapgl";

export interface Painter {
  /** The colour map: 256 RGB triples. */
  setColormap(lut: number[]): void;
  draw(h: MapHeader, values: Float32Array): void;
}

let webgl2: boolean | undefined;

/** Whether WebGL2 works, tried once on a spare canvas: a canvas that gave a WebGL
 * context cannot give a 2D one. */
function hasWebgl2(): boolean {
  if (webgl2 === undefined) {
    try {
      new MapGL(document.createElement("canvas"));
      webgl2 = true;
    } catch (e) {
      console.warn(`nqv: maps drawn without WebGL2 (${e})`);
      webgl2 = false;
    }
  }
  return webgl2;
}

export function painter(canvas: HTMLCanvasElement): Painter {
  return hasWebgl2() ? new MapGL(canvas) : new MapCanvas(canvas);
}
