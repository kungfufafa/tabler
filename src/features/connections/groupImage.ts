import { isGroupImage, MAX_GROUP_IMAGE_CHARS } from "./groups";

/**
 * A group's own image (TablePlus: "New group… (name + icon/image)"). The user picks a picture;
 * it is drawn onto a 64x64 transparent canvas (contained, never cropped) and kept as a PNG data
 * URI in the group's record (`GroupRecord.image`), so it travels in the settings and in exported
 * connection files like the icon id does.
 */

/** Side of the square the picture is drawn into, in pixels. */
export const GROUP_IMAGE_SIZE = 64;
/** The largest picture file taken (larger ones are refused before they are read). */
export const MAX_GROUP_IMAGE_SOURCE_BYTES = 2 * 1024 * 1024;
/** The picture types of the file chooser. */
export const GROUP_IMAGE_TYPES: readonly string[] = [
  "image/png",
  "image/jpeg",
  "image/webp",
  "image/svg+xml",
  "image/gif",
];
export const GROUP_IMAGE_ACCEPT = GROUP_IMAGE_TYPES.join(",");

/** A picture that cannot become a group image; the message is for the user. */
export class GroupImageError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "GroupImageError";
  }
}

/** The 2D context calls `resizeToDataUri` makes. */
export interface DrawContext {
  clearRect: (x: number, y: number, w: number, h: number) => void;
  drawImage: (source: CanvasImageSource, x: number, y: number, w: number, h: number) => void;
}

/** What `resizeToDataUri` needs of a canvas. */
export interface CanvasLike {
  getContext: (kind: "2d") => DrawContext | null;
  toDataURL: (type: "image/png") => string;
}

/** Where a `width` x `height` picture lands on a `size` x `size` canvas: contained, centred. */
export function containRect(
  width: number,
  height: number,
  size: number = GROUP_IMAGE_SIZE,
): { x: number; y: number; width: number; height: number } {
  if (!(width > 0) || !(height > 0)) return { x: 0, y: 0, width: size, height: size };
  const scale = Math.min(size / width, size / height);
  const [w, h] = [width * scale, height * scale];
  return { x: (size - w) / 2, y: (size - h) / 2, width: w, height: h };
}

/** A real canvas of `size` x `size` (a browser). */
function documentCanvas(size: number): CanvasLike {
  const canvas = document.createElement("canvas");
  canvas.width = size;
  canvas.height = size;
  return canvas;
}

/**
 * Draws `source` (`width` x `height`) onto a transparent `size` x `size` canvas, contained and
 * centred, and returns it as a PNG data URI. `canvas` makes the canvas (a stub in tests).
 */
export function resizeToDataUri(
  source: CanvasImageSource,
  width: number,
  height: number,
  {
    size = GROUP_IMAGE_SIZE,
    canvas = documentCanvas,
  }: { size?: number; canvas?: (size: number) => CanvasLike } = {},
): string {
  const surface = canvas(size);
  const ctx = surface.getContext("2d");
  if (!ctx) throw new GroupImageError("This window cannot draw the image");
  ctx.clearRect(0, 0, size, size);
  const r = containRect(width, height, size);
  ctx.drawImage(source, r.x, r.y, r.width, r.height);
  return surface.toDataURL("image/png");
}

/** A picture read from a file, ready to be drawn. */
export interface DecodedImage {
  source: CanvasImageSource;
  width: number;
  height: number;
}

/** A file's bytes as a `data:` URI. */
export function readAsDataUri(file: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () =>
      typeof reader.result === "string"
        ? resolve(reader.result)
        : reject(new Error("The image could not be read"));
    reader.onerror = () => reject(reader.error ?? new Error("The image could not be read"));
    reader.readAsDataURL(file);
  });
}

/**
 * Reads a picture file with an `<img>` (every type, SVG included). The picture reaches the
 * `<img>` as a `data:` URI, never as a `blob:` object URL: the production CSP
 * (`img-src 'self' data: asset: http://asset.localhost`, src-tauri/tauri.conf.json) blocks
 * `blob:` images, so a blob URL would fail to load in the packaged app.
 */
export async function decodeImageFile(file: Blob): Promise<DecodedImage> {
  const uri = await readAsDataUri(file);
  const img = new Image();
  await new Promise<void>((resolve, reject) => {
    img.onload = () => resolve();
    img.onerror = () => reject(new Error("The image could not be decoded"));
    img.src = uri;
  });
  // An SVG without a size of its own reports none: it fills the square.
  const width = img.naturalWidth || GROUP_IMAGE_SIZE;
  const height = img.naturalHeight || GROUP_IMAGE_SIZE;
  return { source: img, width, height };
}

/**
 * The group image of a picture file: a PNG data URI of the picture drawn onto 64x64. Refuses (a
 * `GroupImageError` with the reason) a file of another type, one over 2 MB, one that cannot be
 * read, and a result over the stored size cap (24 KB).
 */
export async function loadGroupImage(
  file: Pick<File, "type" | "size"> & Blob,
  deps: {
    decode?: (file: Blob) => Promise<DecodedImage>;
    canvas?: (size: number) => CanvasLike;
  } = {},
): Promise<string> {
  if (!GROUP_IMAGE_TYPES.includes(file.type))
    throw new GroupImageError("Choose a PNG, JPEG, WebP, SVG or GIF image");
  if (file.size > MAX_GROUP_IMAGE_SOURCE_BYTES)
    throw new GroupImageError("Choose an image smaller than 2 MB");
  let decoded: DecodedImage;
  try {
    decoded = await (deps.decode ?? decodeImageFile)(file);
  } catch {
    throw new GroupImageError("That image could not be read");
  }
  let uri: string;
  try {
    uri = resizeToDataUri(decoded.source, decoded.width, decoded.height, { canvas: deps.canvas });
  } catch (e) {
    // A canvas that cannot give the picture back (a tainted one, no 2D context).
    throw e instanceof GroupImageError ? e : new GroupImageError("That image could not be drawn");
  }
  const chars = uri.length;
  if (!isGroupImage(uri))
    throw new GroupImageError(
      `That image is too detailed to keep (${Math.ceil(chars / 1024)} KB; the limit is ${MAX_GROUP_IMAGE_CHARS / 1024} KB): choose a simpler one`,
    );
  return uri;
}
