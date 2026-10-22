// Generate independent effect oracles from the pinned Scratch renderer, not MSBJIT code.
const fs = require("node:fs"),
  path = require("node:path"),
  vm = require("node:vm");
const { spawn } = require("node:child_process");
const root = path.resolve(__dirname, "../local/scratch-reference"),
  out = path.resolve(__dirname, "../tests/fixtures/effects");
const source = "f969060c1706f80264f032822471090013d9886d";
const pause = (ms) => new Promise((r) => setTimeout(r, ms));
function moduleFrom(name, require) {
  const ctx = { module: { exports: {} }, require, Math };
  vm.runInNewContext(fs.readFileSync(path.join(root, name), "utf8"), ctx);
  return ctx.module.exports;
}
function buildCases() {
  const manager = moduleFrom("ShaderManager.js", () => ({}));
  const transform = moduleFrom("EffectTransform.js", (name) =>
    name.includes("ShaderManager")
      ? manager
      : name === "twgl.js"
        ? {
            v3: {
              copy: (src, dst) => {
                dst.set(src);
              },
            },
          }
        : {},
  );
  const names = Object.keys(manager.EFFECT_INFO);
  const cases = [{ name: "none", values: [0, 0, 0, 0, 0, 0, 0] }];
  for (const [name, values] of Object.entries({
    color: [-450, 50, 200],
    fisheye: [-100, 50, 200],
    whirl: [-180, 180, 720],
    pixelate: [-25, 1000],
    mosaic: [5, 50, 10000],
    brightness: [-100, 25, 100],
    ghost: [50, 100],
  })) {
    for (const v of values) {
      const raw = Array(7).fill(0);
      raw[names.indexOf(name)] = v;
      cases.push({ name: `${name}_${v}`, values: raw });
    }
  }
  cases.push(
    { name: "combined", values: [-33, 90, -180, 25, 30, 20, 35] },
    {
      name: "bitmap_resolution2",
      values: [0, 0, 70, 35, 0, 0, 0],
      resolution: 2,
    },
    { name: "svg_combined", values: [60, 50, 130, 25, 10, -20, 25], svg: true },
  );
  const points = [
    [0, 0],
    [0.13, 0.27],
    [0.5, 0.5],
    [0.85, 0.12],
    [1, 1],
  ];
  for (const c of cases) {
    c.resolution ??= 1;
    c.mask = c.values.reduce((m, v, i) => m | (v !== 0 ? 1 << i : 0), 0);
    c.uniforms = Object.fromEntries(
      names.map((n, i) => [
        manager.EFFECT_INFO[n].uniformName,
        manager.EFFECT_INFO[n].converter(c.values[i]),
      ]),
    );
    c.points = points.map((uv) => {
      const dst = new Float32Array(3);
      transform.transformPoint(
        {
          enabledEffects: c.mask,
          getUniforms: () => c.uniforms,
          skin: {
            getUniforms: () => ({
              u_skinSize: [64 / c.resolution, 64 / c.resolution],
            }),
          },
        },
        [...uv, 0],
        dst,
      );
      return {
        uv,
        expected: Array.from(dst)
          .slice(0, 2)
          .map((v) => (Number.isFinite(v) ? v : null)),
      };
    });
  }
  return { names, cases };
}
async function main() {
  fs.mkdirSync(root, { recursive: true });
  fs.mkdirSync(out, { recursive: true });
  for (const name of [
    "ShaderManager.js",
    "EffectTransform.js",
    "shaders/sprite.frag",
  ]) {
    const url = `https://raw.githubusercontent.com/scratchfoundation/scratch-editor/${source}/packages/scratch-render/src/${name}`;
    const response = await fetch(url);
    if (!response.ok) throw Error(`${url}: ${response.status}`);
    fs.writeFileSync(
      path.join(root, path.basename(name)),
      await response.text(),
    );
  }
  const { names, cases } = buildCases();
  fs.writeFileSync(path.join(out, "skin.png"), png(Buffer.from(skin())));
  const profile = path.join(root, "browser-profile-" + Date.now());
  const child = spawn(
    process.env.EFFECT_BROWSER ||
      "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe",
    [
      "--headless",
      "--no-first-run",
      "--no-default-browser-check",
      "--remote-debugging-port=0",
      "--use-angle=swiftshader",
      "--enable-unsafe-swiftshader",
      "--user-data-dir=" + profile,
      "about:blank",
    ],
    { windowsHide: true, stdio: "ignore" },
  );
  let ws;
  try {
    let port;
    for (let i = 0; i < 200; i++) {
      try {
        port = fs
          .readFileSync(path.join(profile, "DevToolsActivePort"), "utf8")
          .split("\n")[0];
        break;
      } catch {
        await pause(50);
      }
    }
    if (!port) throw Error("Browser failed to start");
    const tabs = await (
      await fetch(`http://127.0.0.1:${port}/json/list`)
    ).json();
    ws = new WebSocket(
      tabs.find((t) => t.type === "page").webSocketDebuggerUrl,
    );
    await new Promise((r, j) => {
      ws.onopen = r;
      ws.onerror = j;
    });
    let id = 0;
    const pending = new Map();
    ws.onmessage = (e) => {
      const m = JSON.parse(e.data);
      if (m.id) {
        const p = pending.get(m.id);
        pending.delete(m.id);
        if (!p) return;
        m.error ? p.reject(m.error) : p.resolve(m.result);
      }
    };
    const send = (method, params) =>
      new Promise((resolve, reject) => {
        const key = ++id;
        pending.set(key, { resolve, reject });
        ws.send(JSON.stringify({ id: key, method, params }));
      });
    const data = {
      cases,
      pixels: skin(),
      frag: fs.readFileSync(path.join(root, "sprite.frag"), "utf8"),
      names,
    };
    const result = await send("Runtime.evaluate", {
      expression: `(${render.toString()})(${JSON.stringify(data)})`,
      returnByValue: true,
    });
    if (result.exceptionDetails)
      throw Error(JSON.stringify(result.exceptionDetails));
    for (const frame of result.result.value)
      fs.writeFileSync(
        path.join(out, frame.name + ".png"),
        png(Buffer.from(frame.rgba, "base64")),
      );
    fs.writeFileSync(
      path.join(out, "reference.json"),
      JSON.stringify(
        {
          source: "scratch-editor f969060c1706f80264f032822471090013d9886d",
          cases,
        },
        null,
        2,
      ),
    );
    console.log("Generated", cases.length, "WebGL shader references");
  } finally {
    if (ws?.readyState === WebSocket.OPEN) {
      ws.send(JSON.stringify({ id: 999999, method: "Browser.close" }));
      await pause(300);
    }
    ws?.close();
    child.kill();
  }
}
function render({ cases, pixels, frag, names }) {
  const canvas = document.createElement("canvas");
  canvas.width = canvas.height = 64;
  const gl = canvas.getContext("webgl", {
    alpha: false,
    preserveDrawingBuffer: true,
  });
  if (!gl) throw Error("No WebGL");
  const shader = (type, src) => {
    const s = gl.createShader(type);
    gl.shaderSource(s, src);
    gl.compileShader(s);
    if (!gl.getShaderParameter(s, gl.COMPILE_STATUS))
      throw Error(gl.getShaderInfoLog(s));
    return s;
  };
  const vertex = shader(
    gl.VERTEX_SHADER,
    "attribute vec2 a;varying vec2 v_texCoord;void main(){gl_Position=vec4(a,0.,1.);v_texCoord=vec2((a.x+1.)*.5,(1.-a.y)*.5);}",
  );
  const texture = gl.createTexture();
  gl.bindTexture(gl.TEXTURE_2D, texture);
  const rgba = new Uint8Array(pixels);
  for (let i = 0; i < rgba.length; i += 4)
    for (let c = 0; c < 3; c++)
      rgba[i + c] = Math.floor((rgba[i + c] * rgba[i + 3] + 127) / 255);
  gl.texImage2D(
    gl.TEXTURE_2D,
    0,
    gl.RGBA,
    64,
    64,
    0,
    gl.RGBA,
    gl.UNSIGNED_BYTE,
    rgba,
  );
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
  const buffer = gl.createBuffer();
  gl.bindBuffer(gl.ARRAY_BUFFER, buffer);
  gl.bufferData(
    gl.ARRAY_BUFFER,
    new Float32Array([-1, -1, 1, -1, -1, 1, -1, 1, 1, -1, 1, 1]),
    gl.STATIC_DRAW,
  );
  gl.enable(gl.BLEND);
  gl.blendFunc(gl.ONE, gl.ONE_MINUS_SRC_ALPHA);
  gl.viewport(0, 0, 64, 64);
  return cases.map((c) => {
    const program = gl.createProgram();
    gl.attachShader(program, vertex);
    gl.attachShader(
      program,
      shader(
        gl.FRAGMENT_SHADER,
        names
          .map((name, i) =>
            c.values[i] !== 0 ? "#define ENABLE_" + name + "\n" : "",
          )
          .join("") + frag,
      ),
    );
    gl.linkProgram(program);
    if (!gl.getProgramParameter(program, gl.LINK_STATUS))
      throw Error(gl.getProgramInfoLog(program));
    gl.useProgram(program);
    const a = gl.getAttribLocation(program, "a");
    gl.enableVertexAttribArray(a);
    gl.vertexAttribPointer(a, 2, gl.FLOAT, false, 0, 0);
    for (const [name, value] of Object.entries(c.uniforms))
      gl.uniform1f(gl.getUniformLocation(program, name), value);
    gl.uniform2f(
      gl.getUniformLocation(program, "u_skinSize"),
      64 / c.resolution,
      64 / c.resolution,
    );
    gl.uniform1i(gl.getUniformLocation(program, "u_skin"), 0);
    const filter = c.svg && c.mask & 30 ? gl.LINEAR : gl.NEAREST;
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, filter);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, filter);
    gl.clearColor(1, 1, 1, 1);
    gl.clear(gl.COLOR_BUFFER_BIT);
    gl.drawArrays(gl.TRIANGLES, 0, 6);
    const bottom = new Uint8Array(64 * 64 * 4);
    gl.readPixels(0, 0, 64, 64, gl.RGBA, gl.UNSIGNED_BYTE, bottom);
    const top = new Uint8Array(bottom.length);
    for (let y = 0; y < 64; y++)
      top.set(bottom.subarray(y * 256, (y + 1) * 256), (63 - y) * 256);
    return { name: c.name, rgba: btoa(String.fromCharCode(...top)) };
  });
}
function skin() {
  const colors = [
    [255, 0, 0],
    [0, 255, 0],
    [0, 0, 255],
    [255, 255, 0],
    [0, 255, 255],
    [255, 0, 255],
    [255, 255, 255],
    [128, 128, 128],
    [0, 0, 0],
  ];
  const result = [];
  for (let y = 0; y < 64; y++)
    for (let x = 0; x < 64; x++)
      result.push(
        ...colors[(Math.floor(x / 8) + Math.floor(y / 8) * 3) % 9],
        [255, 128, 64, 0][(Math.floor(x / 16) + Math.floor(y / 16)) % 4],
      );
  return result;
}
function png(rgba) {
  const { deflateSync, crc32 } = require("node:zlib");
  const chunk = (tag, data) => {
    const body = Buffer.concat([Buffer.from(tag), data]),
      result = Buffer.alloc(body.length + 8);
    result.writeUInt32BE(data.length, 0);
    body.copy(result, 4);
    result.writeUInt32BE(crc32(body), result.length - 4);
    return result;
  };
  const header = Buffer.alloc(13);
  header.writeUInt32BE(64, 0);
  header.writeUInt32BE(64, 4);
  header[8] = 8;
  header[9] = 6;
  const rows = Buffer.alloc(64 * 257);
  for (let y = 0; y < 64; y++)
    rgba.copy(rows, y * 257 + 1, y * 256, (y + 1) * 256);
  return Buffer.concat([
    Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]),
    chunk("IHDR", header),
    chunk("IDAT", deflateSync(rows)),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}
main().catch((e) => {
  console.error(e);
  process.exitCode = 1;
});
