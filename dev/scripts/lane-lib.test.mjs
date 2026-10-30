// dev/scripts/lane-lib.test.mjs -- tests of the lane tooling: node --test dev/scripts/lane-lib.test.mjs
// Locks and snapshots go to a temporary slot root, never C:/t.

import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { after, describe, it } from "node:test";

const TMP = fs.mkdtempSync(path.join(os.tmpdir(), "tabler-lanes-"));
process.env.TABLER_SLOT_ROOT = TMP;
const lib = await import("./lane-lib.mjs");
after(() => {
  lib.releaseAll();
  fs.rmSync(TMP, { recursive: true, force: true });
});

const lanes = lib.readLanes();

describe("ownership", () => {
  it("matches folder, file and name globs", () => {
    const { globMatches } = lib;
    assert.ok(globMatches("src-tauri/src/drivers/redis/mod.rs", "src-tauri/src/drivers/redis/**"));
    assert.ok(!globMatches("src-tauri/src/drivers/redis.rs", "src-tauri/src/drivers/redis/**"));
    assert.ok(!globMatches("src-tauri/src/drivers/redisx/a.rs", "src-tauri/src/drivers/redis/**"));
    assert.ok(globMatches("src/lib/ipc/bindings/RedisParams.ts", "src/lib/ipc/bindings/Redis*.ts"));
    assert.ok(!globMatches("src/lib/ipc/bindings/lex/Redis.ts", "src/lib/ipc/bindings/Redis*.ts"));
    assert.ok(
      !globMatches("src/lib/ipc/bindings/RedisParams.json", "src/lib/ipc/bindings/Redis*.ts"),
    );
    assert.ok(globMatches("docs/drivers/redis.md", "docs/drivers/redis.md"));
    assert.ok(!globMatches("docs/drivers/redis.md.bak", "docs/drivers/redis.md"));
  });

  it("gives each lane its files and leaves shared ones to the foundation", () => {
    const owner = (f) => lib.ownerOf(lanes, f);
    assert.equal(owner("src-tauri/src/drivers/redis/entry.rs"), "redis");
    assert.equal(owner("src-tauri/src/sqlgen/native/redis.rs"), "redis");
    assert.equal(owner("src-tauri/src/drivers/sqlite_http/d1/mod.rs"), "sqlite-remote");
    assert.equal(owner("src-tauri/src/drivers/postgres/flavor/imp/mod.rs"), "pg-flavors");
    assert.equal(owner("src-tauri/src/sqlgen/dialect/tsql.rs"), "sqlserver");
    assert.equal(owner("src-tauri/tests/pg_flavors.rs"), "pg-flavors");
    assert.equal(owner("src/features/drivers/mongodb/index.ts"), "mongodb");
    assert.equal(owner("src/lib/ipc/mock/drivers/duckdb/index.ts"), "duckdb");
    assert.equal(owner("src/lib/ipc/bindings/MssqlParams.ts"), "sqlserver");
    assert.equal(owner("src/lib/ipc/bindings/lex/redshift.json"), "pg-flavors");
    assert.equal(owner("docs/drivers/elasticsearch.md"), "elasticsearch");
    for (const shared of [
      "src-tauri/src/lib.rs",
      "src-tauri/src/drivers/mod.rs",
      "src-tauri/src/drivers/postgres/flavor/mod.rs",
      "src-tauri/src/drivers/registry.rs",
      "src-tauri/src/sqlgen/dialect/mod.rs",
      "src-tauri/src/sqlgen/native/mod.rs",
      "src-tauri/Cargo.toml",
      "src-tauri/tests/common/mod.rs",
      "src/features/drivers/catalog.ts",
      "src/lib/ipc/bindings/Driver.ts",
      "src/lib/ipc/bindings/driverCatalog.ts",
      "docs/drivers/README.md",
      "dev/lanes.json",
    ]) {
      assert.equal(owner(shared), null, shared);
    }
  });

  it("owns no file twice", () => {
    // A path standing for each glob must belong to exactly one lane.
    for (const [name, lane] of Object.entries(lanes.lanes)) {
      for (const glob of lib.ownedGlobs(lane)) {
        const sample = glob.endsWith("/**") ? `${glob.slice(0, -3)}/x.rs` : glob.replace("*", "X");
        const owners = Object.entries(lanes.lanes)
          .filter(([, l]) => lib.ownedGlobs(l).some((g) => lib.globMatches(sample, g)))
          .map(([n]) => n);
        assert.deepEqual(owners, [name], `${sample}`);
      }
    }
  });

  it("puts every glob where its kind lives, with a brief, a slot and a suite per variable", () => {
    for (const [name, lane] of Object.entries(lanes.lanes)) {
      assert.ok(["s1", "s2", "s3"].includes(lane.slot), `${name} slot`);
      assert.ok(lane.features.length > 0, `${name} features`);
      assert.ok(
        lane.rust.every((g) => g.startsWith("src-tauri/")),
        `${name} rust`,
      );
      assert.ok(
        lane.ts.every((g) => g.startsWith("src/") && g.endsWith("/**")),
        `${name} ts`,
      );
      assert.ok(
        lane.bindings.every((g) => g.startsWith("src/lib/ipc/bindings/")),
        `${name}`,
      );
      assert.deepEqual(lane.docs, [`docs/drivers/${name}.md`]);
      assert.ok(fs.existsSync(`${lib.ROOT}/docs/drivers/${name}.md`), `${name} brief`);
      const suites = lane.rust.filter((g) => /^src-tauri\/tests\/\w+\.rs$/.test(g));
      assert.ok(suites.length > 0, `${name} has a real-server suite`);
      const text = suites.map((s) => fs.readFileSync(`${lib.ROOT}/${s}`, "utf8")).join("\n");
      for (const variable of lane.env) {
        assert.ok(text.includes(`"set ${variable}"`), `${name}: a test for ${variable}`);
      }
    }
    for (const file of lanes.smoke) assert.ok(fs.existsSync(`${lib.ROOT}/${file}`), file);
  });

  it("runs the shared integration suites by name, never a lane's", () => {
    const suites = lib.sharedSuites(lanes);
    for (const shared of ["driver_guard", "manifest_features", "postgres", "ssh_tunnel"]) {
      assert.ok(suites.includes(shared), shared);
    }
    const laneSuites = Object.values(lanes.lanes).flatMap((l) =>
      l.rust.flatMap((g) => /^src-tauri\/tests\/(\w+)\.rs$/.exec(g)?.[1] ?? []),
    );
    assert.equal(laneSuites.length, 14);
    for (const suite of laneSuites) assert.ok(!suites.includes(suite), suite);
    assert.ok(!suites.includes("common"));
  });

  it("knows which shared files a build reads", () => {
    assert.ok(lib.isBuildInput("src-tauri/src/lib.rs"));
    assert.ok(lib.isBuildInput("src/app/App.tsx"));
    assert.ok(lib.isBuildInput("package.json"));
    assert.ok(lib.isBuildInput("dev/lanes.json"));
    assert.ok(!lib.isBuildInput("docs/drivers/README.md"));
    assert.ok(!lib.isBuildInput("AGENTS.md"));
    assert.ok(!lib.isBuildInput("dev/scripts/lane.mjs"));
  });
});

describe("parsers", () => {
  it("reads git status -z, renames included", () => {
    const text = " M src/a.ts\0R  new.rs\0old.rs\0?? docs/x.md\0D  gone.ts\0";
    const status = lib.parseStatus(text);
    assert.deepEqual(status, [
      { x: " ", y: "M", path: "src/a.ts" },
      { x: "R", y: " ", path: "new.rs", from: "old.rs" },
      { x: "?", y: "?", path: "docs/x.md" },
      { x: "D", y: " ", path: "gone.ts" },
    ]);
    assert.deepEqual(status.flatMap(lib.statusPaths), [
      "src/a.ts",
      "new.rs",
      "old.rs",
      "docs/x.md",
      "gone.ts",
    ]);
  });

  it("reads tsc diagnostics with their continuation lines", () => {
    const out = [
      "src/features/drivers/redis/index.ts(3,7): error TS2322: Type 'string' is not assignable to type 'number'.",
      "src\\app\\App.tsx(10,1): error TS2304: Cannot find name 'x'.",
      "  The expected type comes from here.",
      "error TS5083: Cannot read file 'tsconfig.json'.",
      "",
    ].join("\n");
    const d = lib.parseTsc(out);
    assert.equal(d.length, 3);
    assert.equal(d[0].file, "src/features/drivers/redis/index.ts");
    assert.equal(d[0].code, "TS2322");
    assert.equal(d[1].file, "src/app/App.tsx");
    assert.match(d[1].text, /expected type/);
    assert.equal(d[2].file, null);
  });

  it("fails a type error only for the lane whose file holds it", async () => {
    // A project of three files with the repository's TypeScript: the mongodb lane's index has an
    // error, the redis lane's and the shared file have none (the F10 isolation acceptance).
    const root = path.join(TMP, "tsc-project");
    const write = (file, text) => {
      fs.mkdirSync(path.dirname(path.join(root, file)), { recursive: true });
      fs.writeFileSync(path.join(root, file), text);
    };
    write(
      "tsconfig.json",
      JSON.stringify({ compilerOptions: { strict: true, noEmit: true }, include: ["src"] }),
    );
    write("src/shared.ts", "export const ok: number = 1;\n");
    write("src/features/drivers/redis/index.ts", 'export { ok } from "../../../shared";\n');
    write("src/features/drivers/mongodb/index.ts", 'export const broken: number = "x";\n');
    fs.symlinkSync(
      path.join(lib.ROOT, "node_modules"),
      path.join(root, "node_modules"),
      "junction",
    );
    const posixRoot = root.split(path.sep).join("/");

    const redis = await lib.tscCheck(lanes, "redis", posixRoot);
    assert.deepEqual(redis.failing, []);
    assert.deepEqual(redis.ignored, { mongodb: 1 });
    const mongodb = await lib.tscCheck(lanes, "mongodb", posixRoot);
    assert.deepEqual(
      mongodb.failing.map((d) => [d.file, d.code]),
      [["src/features/drivers/mongodb/index.ts", "TS2322"]],
    );
    // The foundation's mode ignores every lane's files; an error in a shared file fails all.
    assert.deepEqual((await lib.tscCheck(lanes, null, posixRoot)).failing, []);
    write("src/shared.ts", 'export const ok: number = "1";\n');
    assert.equal((await lib.tscCheck(lanes, "redis", posixRoot)).failing.length, 1);
  });

  it("reads .env files", () => {
    const file = path.join(TMP, "test.env");
    fs.writeFileSync(
      file,
      '# comment\nA=1\nexport B="two words"\nC=x # note\n# D=hidden\nE=\nbad line\n',
    );
    assert.deepEqual(lib.readEnvFile(file), { A: "1", B: "two words", C: "x", E: "" });
    assert.deepEqual(lib.readEnvFile(path.join(TMP, "missing.env")), {});
  });

  it("splits long file lists", () => {
    const files = Array.from({ length: 10 }, (_, i) => `file-${i}.ts`);
    const parts = lib.batches(files, 40);
    assert.deepEqual(parts.flat(), files);
    assert.ok(parts.length > 1);
    assert.ok(parts.every((p) => p.join("   ").length <= 40));
  });
});

describe("slot locks", () => {
  const deadPid = () => {
    const child = spawnSync(process.execPath, ["-e", ""]);
    return child.pid;
  };

  it("takes a free slot, refuses a held one and releases it", () => {
    assert.equal(lib.tryLock("s1", "test"), null);
    const holder = JSON.parse(fs.readFileSync(lib.lockFile("s1"), "utf8"));
    assert.equal(holder.pid, process.pid);
    assert.equal(lib.tryLock("s1", "again").pid, process.pid);
    lib.release("s1");
    assert.ok(!fs.existsSync(lib.lockFile("s1")));
  });

  it("removes a stale lock whose process is gone", () => {
    const file = lib.lockFile("s2");
    fs.mkdirSync(path.dirname(file), { recursive: true });
    fs.writeFileSync(file, JSON.stringify({ pid: deadPid(), what: "crashed" }));
    assert.equal(lib.lockHolder(file), null);
    assert.equal(lib.tryLock("s2", "test"), null);
    lib.release("s2");
  });

  it("never removes a lock another live process holds", () => {
    const file = lib.lockFile("s3");
    fs.mkdirSync(path.dirname(file), { recursive: true });
    fs.writeFileSync(file, JSON.stringify({ pid: process.ppid, what: "other" }));
    lib.release("s3");
    assert.ok(fs.existsSync(file));
    assert.equal(lib.tryLock("s3", "test").what, "other");
    fs.rmSync(file);
  });

  it("takes the first free slot and waits while all are busy", async () => {
    const other = lib.lockFile("s1");
    fs.mkdirSync(path.dirname(other), { recursive: true });
    fs.writeFileSync(other, JSON.stringify({ pid: process.ppid, what: "other" }));
    const quiet = { pollMs: 20, log: () => {} };
    assert.equal(await lib.acquireAny(["s1", "s2"], "test", quiet), "s2");
    setTimeout(() => fs.rmSync(other), 100);
    assert.equal(await lib.acquireAny(["s1"], "test", quiet), "s1");
    lib.releaseAll();
  });

  it("keeps the TS slots apart from the build slots", () => {
    assert.match(lib.lockFile("ts1"), /\/locks\/ts1\.lock$/);
    assert.match(lib.lockFile("s0"), /\/s0\/\.tabler-lane\.lock$/);
    assert.equal(lib.slotDir("s2"), `${TMP.split(path.sep).join("/")}/s2`);
  });
});

describe("cargo environment", () => {
  it("builds a lane in its slot with its own bindings folder", () => {
    const env = lib.cargoEnv("s3", { jobs: 4, lane: "redis" });
    assert.equal(env.CARGO_TARGET_DIR, lib.slotDir("s3"));
    assert.equal(env.CARGO_BUILD_JOBS, "4");
    assert.equal(env.TS_RS_EXPORT_DIR, `${lib.slotDir("s3")}/bindings-redis`);
    assert.equal(env.CARGO_PROFILE_DEV_DEBUG, "line-tables-only");
    const gate = lib.cargoEnv("s0", { jobs: 8, laneSlot: false });
    assert.equal(gate.TS_RS_EXPORT_DIR, process.env.TS_RS_EXPORT_DIR);
    assert.equal(gate.CARGO_PROFILE_DEV_DEBUG, process.env.CARGO_PROFILE_DEV_DEBUG);
    const redis = lib.getLane(lanes, "redis");
    assert.deepEqual(lib.laneFeatureArgs(redis).slice(2), [
      "--locked",
      "--no-default-features",
      "--features",
      "drv-redis",
    ]);
    assert.throws(() => lib.getLane(lanes, "oracle"), lib.UsageError);
  });
});

describe("rustfmt", () => {
  it("formats each file alone, without the modules it declares", async () => {
    const root = path.join(TMP, "fmt");
    fs.mkdirSync(path.join(root, "src-tauri"), { recursive: true });
    // `mod lane;` names a file that does not exist: following it would fail.
    fs.writeFileSync(path.join(root, "ok.rs"), '#[cfg(feature = "x")]\nmod lane;\n\nfn a() {}\n');
    fs.writeFileSync(path.join(root, "messy.rs"), "fn  b( ) {\r\n}\r\n");
    fs.writeFileSync(path.join(root, "broken.rs"), "fn c( {\n");
    const r = await lib.rustfmtFiles(["ok.rs", "messy.rs", "broken.rs"], { root });
    assert.deepEqual(r.changed, ["messy.rs"]);
    assert.deepEqual(
      r.failed.map((f) => f.file),
      ["broken.rs"],
    );
    await lib.rustfmtFiles(["messy.rs"], { root, write: true });
    assert.equal(fs.readFileSync(path.join(root, "messy.rs"), "utf8"), "fn b() {}\r\n");
  });
});

describe("command lines", () => {
  const script = (name, ...args) =>
    spawnSync(process.execPath, [`${lib.ROOT}/dev/scripts/${name}`, ...args], {
      cwd: lib.ROOT,
      encoding: "utf8",
    });

  it("refuses to commit a shared file or another lane's for a lane", () => {
    // Refused before anything is added or committed: only `git status` runs.
    const shared = script(
      "lane-commit.mjs",
      "redis",
      "-m",
      "redis: x",
      "--",
      "src-tauri/src/lib.rs",
    );
    assert.equal(shared.status, 2, shared.stderr);
    assert.match(shared.stderr, /refused: not owned by the redis lane/);
    assert.match(shared.stderr, /src-tauri\/src\/lib\.rs \(shared\)/);
    const other = script("lane-commit.mjs", "redis", "-m", "x", "--", "src-tauri/tests/mongodb.rs");
    assert.equal(other.status, 2, other.stderr);
    assert.match(other.stderr, /src-tauri\/tests\/mongodb\.rs \(mongodb\)/);
  });

  it("rejects unknown lanes and commands", () => {
    const lane = script("lane.mjs", "oracle", "check");
    assert.equal(lane.status, 2);
    assert.match(lane.stderr, /unknown lane "oracle"/);
    const command = script("lane.mjs", "redis", "build");
    assert.equal(command.status, 2);
    assert.match(command.stderr, /unknown command build/);
    assert.equal(script("tsc-lane.mjs").status, 2);
    assert.equal(script("gates.mjs", "everything").status, 2);
  });
});

describe("snapshot", () => {
  it("copies HEAD with the overlay's files over it", async () => {
    const dir = await lib.snapshot("test", ["dev/lanes.json", "dev/not-a-file.txt"]);
    assert.ok(fs.existsSync(`${dir}/src-tauri/Cargo.toml`));
    assert.deepEqual(
      JSON.parse(fs.readFileSync(`${dir}/dev/lanes.json`, "utf8")),
      JSON.parse(fs.readFileSync(`${lib.ROOT}/dev/lanes.json`, "utf8")),
    );
    assert.ok(!fs.existsSync(`${dir}/dev/not-a-file.txt`));
    assert.ok(!fs.existsSync(`${dir}/.git`));
  });
});
