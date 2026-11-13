// dev/scripts/updater-manifest.mjs -- the `latest.json` the in-app updater reads
// (.github/workflows/build.yml, docs/updates.md), from the files a release publishes:
//
//   node dev/scripts/updater-manifest.mjs --dir <files> --version <version> \
//        --base-url https://github.com/<owner>/<repo>/releases/download/<tag> \
//        [--notes-file <file>] [--out <files>/latest.json]
//
// Each updater bundle has its signature beside it (`<file>.sig`, written by `tauri build` with
// the signing key): the macOS `.app.tar.gz` (one universal build, listed for both
// architectures), the NSIS `-setup.exe` and the `.msi`. Without any signature (a build made
// without the key) nothing is written and the release goes out without `latest.json`.
//
// A signature must name the version it was made for, and that version must be the manifest's:
// the app refuses anything else (`requireSignedVersion`), so a mismatch stops the release here.

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

/** The updater targets a file serves, by its name (`{os}-{arch}[-{installer}]`, most specific first). */
export function targetsOf(name) {
  if (name.endsWith(".app.tar.gz"))
    return ["darwin-aarch64-app", "darwin-x86_64-app", "darwin-aarch64", "darwin-x86_64"];
  // The plain key is what a build that cannot tell its installer asks for: the setup .exe.
  if (name.endsWith("-setup.exe")) return ["windows-x86_64-nsis", "windows-x86_64"];
  if (name.endsWith(".msi")) return ["windows-x86_64-msi"];
  return [];
}

/**
 * The version a signature was made for: the `version:` field of its trusted comment (the `.sig`
 * is base64 of the minisign text); null when it has none.
 */
export function signedVersion(signature) {
  const text = Buffer.from(signature.trim(), "base64").toString("utf8");
  const comment = text.split("\n").find((line) => line.startsWith("trusted comment:"));
  if (!comment) return null;
  const field = comment
    .slice("trusted comment:".length)
    .trim()
    .split("\t")
    .find((f) => f.startsWith("version:"));
  return field ? field.slice("version:".length) : null;
}

/**
 * The manifest of `files` (`{ name, signature }`, the signature being the `.sig` file's text),
 * or null when none of them is an updater bundle.
 */
export function buildManifest({ version, notes, pubDate, baseUrl, files }) {
  const platforms = {};
  for (const { name, signature } of [...files].sort((a, b) => a.name.localeCompare(b.name))) {
    const targets = targetsOf(name);
    if (targets.length === 0) continue;
    const signed = signedVersion(signature);
    if (signed === null) throw new Error(`${name}.sig names no version: sign it with tauri build`);
    if (signed !== version)
      throw new Error(`${name} was signed for ${signed}, but the release is ${version}`);
    const entry = {
      signature: signature.trim(),
      url: `${baseUrl.replace(/\/+$/, "")}/${encodeURIComponent(name)}`,
    };
    for (const target of targets) {
      if (platforms[target]) throw new Error(`two files for ${target}: ${name}`);
      platforms[target] = entry;
    }
  }
  if (Object.keys(platforms).length === 0) return null;
  return { version, notes: notes.trim(), pub_date: pubDate, platforms };
}

function main(argv) {
  const arg = (name) => {
    const i = argv.indexOf(name);
    return i >= 0 ? argv[i + 1] : undefined;
  };
  const dir = arg("--dir");
  const version = arg("--version");
  const baseUrl = arg("--base-url");
  if (!dir || !version || !baseUrl)
    throw new Error("usage: updater-manifest.mjs --dir <files> --version <v> --base-url <url>");
  const out = arg("--out") ?? path.join(dir, "latest.json");
  const notesFile = arg("--notes-file");
  const notes = notesFile && fs.existsSync(notesFile) ? fs.readFileSync(notesFile, "utf8") : "";
  const names = fs.readdirSync(dir);
  const files = names
    .filter((name) => names.includes(`${name}.sig`))
    .map((name) => ({ name, signature: fs.readFileSync(path.join(dir, `${name}.sig`), "utf8") }));
  const manifest = buildManifest({
    version,
    notes,
    pubDate: new Date().toISOString(),
    baseUrl,
    files,
  });
  if (!manifest) {
    console.log("no signed updater bundles: the release goes out without latest.json");
    return;
  }
  fs.writeFileSync(out, `${JSON.stringify(manifest, null, 2)}\n`);
  console.log(`${out}: ${version} for ${Object.keys(manifest.platforms).join(", ")}`);
}

/** Whether this file is the script node was started with (not imported by a test). */
function isMain() {
  if (!process.argv[1]) return false;
  // Real paths: a temp folder behind a symlink, or a drive letter in another case, is the same file.
  const same = (p) => (process.platform === "win32" ? p.toLowerCase() : p);
  try {
    return (
      same(fs.realpathSync(fileURLToPath(import.meta.url))) ===
      same(fs.realpathSync(process.argv[1]))
    );
  } catch {
    return false;
  }
}

if (isMain()) {
  try {
    main(process.argv.slice(2));
  } catch (e) {
    console.error(`updater-manifest: ${e.message}`);
    process.exit(1);
  }
}
