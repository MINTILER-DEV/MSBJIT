These reference images were rendered by Scratch's unmodified GLSL shader in
headless Chromium WebGL (ANGLE SwiftShader), over a white background. They are
independent of MSBJIT's WGSL and CPU effect implementations. The generated
pattern contains saturated colors, grayscale, black, and four alpha levels.

The pinned reference is scratch-editor commit
`f969060c1706f80264f032822471090013d9886d`:

- [Shader and effect order](https://github.com/scratchfoundation/scratch-editor/blob/f969060c1706f80264f032822471090013d9886d/packages/scratch-render/src/shaders/sprite.frag)
- [Effect units and converters](https://github.com/scratchfoundation/scratch-editor/blob/f969060c1706f80264f032822471090013d9886d/packages/scratch-render/src/ShaderManager.js)
- [CPU coordinate transformations](https://github.com/scratchfoundation/scratch-editor/blob/f969060c1706f80264f032822471090013d9886d/packages/scratch-render/src/EffectTransform.js)

`reference.json` contains input values, uniforms, and coordinates produced by
the reference CPU transform. Its null coordinates mark Scratch's undefined
fisheye calculation exactly at the center; MSBJIT preserves the center there.
`bitmap_resolution2` checks logical skin dimensions. `svg_combined` exercises
linear sampling; bitmap cases use Scratch's nearest sampling.

Regenerate with Node 22.15+ and Edge/Chromium:

```powershell
node scripts/generate_effect_references.cjs
```

Set `EFFECT_BROWSER` to another Chromium executable if needed. Upstream source
and isolated browser profiles are stored under ignored `local/scratch-reference`.
The source is fetched only when regenerating; ordinary tests run offline.

```powershell
cargo test --all-features
cargo test --all-features --lib all_graphic_effects_match_scratch_webgl_reference -- --ignored --nocapture
```

GPU and CPU stamp comparisons allow two 8-bit levels per channel to account
for differences in floating-point operations and framebuffer rounding.
