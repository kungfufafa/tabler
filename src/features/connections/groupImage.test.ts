import { afterEach, describe, expect, it, vi } from "vitest";
import {
  containRect,
  decodeImageFile,
  GROUP_IMAGE_SIZE,
  GroupImageError,
  loadGroupImage,
  MAX_GROUP_IMAGE_SOURCE_BYTES,
  readAsDataUri,
  resizeToDataUri,
  type CanvasLike,
  type DecodedImage,
} from "./groupImage";
import { isGroupImage, MAX_GROUP_IMAGE_CHARS } from "./groups";

const PNG =
  "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGNgYGD4DwABBAEAwS2OUAAAAABJRU5ErkJggg==";

/** A canvas that records what was drawn on it. */
function stubCanvas(uri: string = PNG) {
  const calls: unknown[][] = [];
  const made: number[] = [];
  const ctx = {
    clearRect: (...a: number[]) => void calls.push(["clearRect", ...a]),
    drawImage: (...a: unknown[]) => void calls.push(["drawImage", ...a]),
  };
  const canvas = (size: number): CanvasLike => {
    made.push(size);
    return { getContext: () => ctx, toDataURL: vi.fn(() => uri) };
  };
  return { canvas, calls, made };
}

const file = (type: string, size = 1000) =>
  Object.defineProperty(new File(["x"], "logo", { type }), "size", { value: size });

function decoded(width = 100, height = 50) {
  const image: DecodedImage = { source: {} as CanvasImageSource, width, height };
  return { image, decode: vi.fn(async () => image) };
}

describe("containRect", () => {
  it("fits the picture inside the square without cropping it, centred", () => {
    expect(containRect(128, 64)).toEqual({ x: 0, y: 16, width: 64, height: 32 });
    expect(containRect(64, 128)).toEqual({ x: 16, y: 0, width: 32, height: 64 });
    expect(containRect(64, 64)).toEqual({ x: 0, y: 0, width: 64, height: 64 });
    // Small pictures are scaled up to fill the square (an icon is shown at 14 px anyway).
    expect(containRect(16, 8)).toEqual({ x: 0, y: 16, width: 64, height: 32 });
    expect(containRect(1000, 1000, 32)).toEqual({ x: 0, y: 0, width: 32, height: 32 });
  });

  it("fills the square for a picture with no size", () => {
    expect(containRect(0, 0)).toEqual({ x: 0, y: 0, width: 64, height: 64 });
    expect(containRect(NaN, 10)).toEqual({ x: 0, y: 0, width: 64, height: 64 });
  });
});

describe("resizeToDataUri", () => {
  it("draws the picture contained on a transparent 64x64 canvas and returns a PNG", () => {
    const { canvas, calls, made } = stubCanvas();
    const source = {} as CanvasImageSource;
    expect(resizeToDataUri(source, 200, 100, { canvas })).toBe(PNG);
    expect(made).toEqual([GROUP_IMAGE_SIZE]);
    expect(calls).toEqual([
      ["clearRect", 0, 0, 64, 64],
      ["drawImage", source, 0, 16, 64, 32],
    ]);
    expect(isGroupImage(PNG)).toBe(true);
  });

  it("asks the canvas for a PNG", () => {
    const toDataURL = vi.fn(() => PNG);
    const canvas = (): CanvasLike => ({
      getContext: () => ({ clearRect: () => {}, drawImage: () => {} }),
      toDataURL,
    });
    resizeToDataUri({} as CanvasImageSource, 10, 10, { canvas });
    expect(toDataURL).toHaveBeenCalledWith("image/png");
  });

  it("refuses a canvas without a 2D context", () => {
    const canvas = (): CanvasLike => ({ getContext: () => null, toDataURL: () => PNG });
    expect(() => resizeToDataUri({} as CanvasImageSource, 10, 10, { canvas })).toThrow(
      GroupImageError,
    );
  });
});

describe("loadGroupImage", () => {
  it("turns a picture file into a PNG data URI", async () => {
    const { canvas, calls } = stubCanvas();
    const d = decoded(100, 50);
    for (const type of ["image/png", "image/jpeg", "image/webp", "image/svg+xml", "image/gif"])
      await expect(loadGroupImage(file(type), { decode: d.decode, canvas })).resolves.toBe(PNG);
    expect(d.decode).toHaveBeenCalledTimes(5);
    expect(calls.at(-1)).toEqual(["drawImage", d.image.source, 0, 16, 64, 32]);
  });

  it("refuses other file types", async () => {
    const d = decoded();
    for (const type of ["text/plain", "application/pdf", "image/bmp", "image/tiff", ""])
      await expect(loadGroupImage(file(type), { decode: d.decode })).rejects.toThrow(
        /PNG, JPEG, WebP, SVG or GIF/,
      );
    expect(d.decode).not.toHaveBeenCalled();
  });

  it("refuses a file over 2 MB before reading it", async () => {
    const d = decoded();
    await expect(
      loadGroupImage(file("image/png", MAX_GROUP_IMAGE_SOURCE_BYTES + 1), { decode: d.decode }),
    ).rejects.toThrow(/smaller than 2 MB/);
    expect(d.decode).not.toHaveBeenCalled();
    // Exactly 2 MB is fine.
    await expect(
      loadGroupImage(file("image/png", MAX_GROUP_IMAGE_SOURCE_BYTES), {
        decode: d.decode,
        canvas: stubCanvas().canvas,
      }),
    ).resolves.toBe(PNG);
  });

  it("refuses a picture that cannot be read", async () => {
    const decode = vi.fn(async () => {
      throw new Error("decode failed");
    });
    await expect(loadGroupImage(file("image/png"), { decode })).rejects.toThrow(
      "That image could not be read",
    );
  });

  it("refuses a result over the stored size cap, and says how big it was", async () => {
    const huge = `data:image/png;base64,${"A".repeat(MAX_GROUP_IMAGE_CHARS)}`;
    const d = decoded();
    await expect(
      loadGroupImage(file("image/png"), { decode: d.decode, canvas: stubCanvas(huge).canvas }),
    ).rejects.toThrow(/too detailed to keep \(25 KB; the limit is 24 KB\)/);
  });

  it("refuses a canvas that will not give the picture back (tainted, no context)", async () => {
    const d = decoded();
    const tainted = (): CanvasLike => ({
      getContext: () => ({ clearRect: () => {}, drawImage: () => {} }),
      toDataURL: () => {
        throw new DOMException("tainted", "SecurityError");
      },
    });
    await expect(
      loadGroupImage(file("image/svg+xml"), { decode: d.decode, canvas: tainted }),
    ).rejects.toThrow("That image could not be drawn");
  });
});

/** An `Image` that records what it is given as `src` and loads it (jsdom decodes nothing). */
function stubImage(outcome: "load" | "error" = "load") {
  const sources: string[] = [];
  class FakeImage {
    naturalWidth = 120;
    naturalHeight = 30;
    onload: (() => void) | null = null;
    onerror: (() => void) | null = null;
    set src(value: string) {
      sources.push(value);
      queueMicrotask(() => (outcome === "load" ? this.onload?.() : this.onerror?.()));
    }
  }
  vi.stubGlobal("Image", FakeImage);
  return sources;
}

describe("decodeImageFile (the packaged app's CSP allows data: images, not blob:)", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });

  const original = Object.getOwnPropertyDescriptor(URL, "createObjectURL");
  afterEach(() => {
    if (original) Object.defineProperty(URL, "createObjectURL", original);
    else delete (URL as { createObjectURL?: unknown }).createObjectURL;
  });

  /** Counts the object URLs anything makes (a `blob:` image is blocked by the production CSP). */
  function watchObjectUrls() {
    const create = vi.fn(() => "blob:forbidden");
    Object.defineProperty(URL, "createObjectURL", {
      value: create,
      configurable: true,
      writable: true,
    });
    return create;
  }

  it("reads the file into a data: URI and never makes a blob: URL", async () => {
    const sources = stubImage();
    const create = watchObjectUrls();
    const png = new File([new Uint8Array([137, 80, 78, 71])], "logo.png", { type: "image/png" });
    const decoded = await decodeImageFile(png);
    expect(create).not.toHaveBeenCalled();
    expect(sources).toHaveLength(1);
    expect(sources[0]).toMatch(/^data:image\/png;base64,/);
    expect(sources[0]).not.toMatch(/^blob:/);
    expect(decoded).toMatchObject({ width: 120, height: 30 });
  });

  it("keeps the picture's own type in the URI (SVG too)", async () => {
    const sources = stubImage();
    const svg = new File(["<svg xmlns='http://www.w3.org/2000/svg'/>"], "logo.svg", {
      type: "image/svg+xml",
    });
    await decodeImageFile(svg);
    expect(sources[0]).toMatch(/^data:image\/svg\+xml;base64,/);
    expect(atob(sources[0]!.split(",")[1]!)).toContain("<svg");
  });

  it("takes the whole path of loadGroupImage without a blob: URL", async () => {
    stubImage();
    const create = watchObjectUrls();
    const { canvas } = stubCanvas();
    const gif = new File(["GIF89a"], "logo.gif", { type: "image/gif" });
    await expect(loadGroupImage(gif, { canvas })).resolves.toBe(PNG);
    expect(create).not.toHaveBeenCalled();
  });

  it("an SVG with no size of its own fills the square", async () => {
    class SizelessImage {
      naturalWidth = 0;
      naturalHeight = 0;
      onload: (() => void) | null = null;
      set src(_value: string) {
        queueMicrotask(() => this.onload?.());
      }
    }
    vi.stubGlobal("Image", SizelessImage);
    const svg = new File(["<svg/>"], "logo.svg", { type: "image/svg+xml" });
    await expect(decodeImageFile(svg)).resolves.toMatchObject({
      width: GROUP_IMAGE_SIZE,
      height: GROUP_IMAGE_SIZE,
    });
  });

  it("rejects a picture the browser cannot decode", async () => {
    stubImage("error");
    const png = new File(["not a png"], "logo.png", { type: "image/png" });
    await expect(decodeImageFile(png)).rejects.toThrow(/could not be decoded/);
    await expect(loadGroupImage(png)).rejects.toThrow("That image could not be read");
  });
});

describe("readAsDataUri", () => {
  it("gives the file's bytes as base64 under its own type", async () => {
    const uri = await readAsDataUri(new File(["hello"], "a.png", { type: "image/png" }));
    expect(uri).toBe(`data:image/png;base64,${btoa("hello")}`);
  });
});
