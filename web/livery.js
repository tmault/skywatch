// Provider liveries: every aircraft is painted in the colours of the company
// behind its model (Claude/Anthropic, OpenAI, Google Gemini, ...).
//
// `liveryFor(model)` resolves a raw model id such as "claude-opus-5-5" or
// "opencode-go/gemini-4-argon" to a livery. The painters below turn a livery
// into canvas textures: a wrap-around fuselage skin (belly, cheatline,
// windows, titles, tail swoosh), a fin skin, and a transparent tail logo.
// Logos are simple drawn emblems evoking each brand, not copies of artwork.

import * as THREE from './vendor/three.module.js';

/* ------------------------------------------------------------ catalogue */

const SERIF = '"Iowan Old Style", "Palatino Linotype", Palatino, Georgia, serif';
const SANS = '"Inter", "Helvetica Neue", Arial, sans-serif';

export const TANKER_GREY = {
  body: '#8e97a0',      // matte mid-grey military transport airframe (A400M / MRTT)
  belly: '#6f7882',     // darker tactical underside
  wing: '#848d96',      // matte grey flying surfaces
  engine: '#5c646d',    // darker matte grey engine nacelles & hose pods
  radome: '#454c54',    // dark dielectric nose radome
  antiGlare: '#383e45', // matte charcoal anti-glare panel ahead of cockpit
  panel: 'rgba(26, 32, 38, 0.24)',
  stencil: '#323a42',   // low-visibility military stencil text
  roundelOuter: '#4c555f',
  roundelMid: '#adb6bf',
  roundelCore: '#3f4750',
};

const PROVIDERS = {
  anthropic: {
    title: 'Claude', operator: 'ANTHROPIC', titleFont: `600 1px ${SERIF}`,
    body: '#f4f1ea', belly: '#e6dfd2', wing: '#d9d3c7',
    stripes: ['#d97757'], tail: ['#d97757', '#c15f3c'], swoosh: '#d97757',
    engine: '#d97757', titleColor: '#191919', logo: 'claudeSpark', logoColor: '#f4f1ea',
  },
  openai: {
    title: 'OpenAI', titleFont: `600 1px ${SANS}`,
    body: '#ffffff', belly: '#e4e4e4', wing: '#cfd3d8',
    stripes: ['#0d0d0d'], tail: ['#0d0d0d'], swoosh: '#0d0d0d',
    engine: '#1a1a1a', titleColor: '#0d0d0d', logo: 'blossom', logoColor: '#ffffff',
  },
  google: {
    title: 'Gemini', operator: 'Google', titleFont: `500 1px ${SANS}`,
    body: '#ffffff', belly: '#eef1f6', wing: '#d3d8df',
    stripes: ['#4285f4', '#ea4335', '#fbbc05', '#34a853'],
    tail: ['#4285f4', '#9b72cb', '#d96570'], swoosh: '#4285f4',
    engine: '#4285f4', titleColor: '#4285f4', logo: 'sparkle', logoColor: '#ffffff',
  },
  deepseek: {
    title: 'deepseek', titleFont: `700 1px ${SANS}`,
    body: '#ffffff', belly: '#e8ecff', wing: '#cfd6ea',
    stripes: ['#4d6bfe'], tail: ['#4d6bfe', '#3450e0'], swoosh: '#4d6bfe',
    engine: '#4d6bfe', titleColor: '#4d6bfe', logo: 'whale', logoColor: '#ffffff',
  },
  zhipu: {
    title: 'Z.ai', operator: 'GLM', titleFont: `800 1px ${SANS}`,
    body: '#f5f7fa', belly: '#dfe4ea', wing: '#c9cfd6',
    stripes: ['#111111', '#2d6bff'], tail: ['#111111'], swoosh: '#111111',
    engine: '#111111', titleColor: '#111111', logo: 'zed', logoColor: '#ffffff',
  },
  qwen: {
    title: 'Qwen', titleFont: `700 1px ${SANS}`,
    body: '#ffffff', belly: '#ecebff', wing: '#d2d1ea',
    stripes: ['#615ced'], tail: ['#615ced', '#8b5cf6'], swoosh: '#615ced',
    engine: '#615ced', titleColor: '#615ced', logo: 'hexQ', logoColor: '#ffffff',
  },
  mistral: {
    title: 'Mistral', titleFont: `800 1px ${SANS}`,
    body: '#fffaf0', belly: '#f1e6d2', wing: '#dcd2c0',
    stripes: ['#ffd800', '#ffaf00', '#ff8205', '#fa500f', '#e10500'],
    tail: ['#1b1b1b'], swoosh: '#fa500f',
    engine: '#fa500f', titleColor: '#1b1b1b', logo: 'pixelM', logoColor: '#ffffff',
  },
  xai: {
    title: 'Grok', operator: 'xAI', titleFont: `700 1px ${SANS}`,
    body: '#1a1a1a', belly: '#0c0c0c', wing: '#2a2a2a',
    stripes: ['#ffffff'], tail: ['#000000'], swoosh: '#000000',
    engine: '#000000', titleColor: '#ffffff', logo: 'slash', logoColor: '#ffffff',
  },
  meta: {
    title: 'Llama', operator: 'Meta', titleFont: `700 1px ${SANS}`,
    body: '#ffffff', belly: '#e7effc', wing: '#cfd8e6',
    stripes: ['#0866ff'], tail: ['#0866ff', '#0a4fd1'], swoosh: '#0866ff',
    engine: '#0866ff', titleColor: '#0866ff', logo: 'loop', logoColor: '#ffffff',
  },
  moonshot: {
    title: 'Kimi', operator: 'Moonshot', titleFont: `800 1px ${SANS}`,
    body: '#ffffff', belly: '#e9e9e9', wing: '#cfcfcf',
    stripes: ['#111111'], tail: ['#111111'], swoosh: '#111111',
    engine: '#111111', titleColor: '#111111', logo: 'letter', logoColor: '#ffffff', letter: 'K',
  },
  generic: {
    title: null, titleFont: `700 1px ${SANS}`,
    body: '#eceff3', belly: '#d5dae1', wing: '#c5ccd5',
    stripes: ['#5be7ff'], tail: ['#2a3440'], swoosh: '#2a3440',
    engine: '#2a3440', titleColor: '#2a3440', logo: 'letter', logoColor: '#5be7ff',
  },
};

const RULES = [
  [/claude|anthropic|opus|sonnet|haiku|fable/, 'anthropic'],
  [/gemini|gemma|google|palm|bard/, 'google'],
  [/(^|[^a-z])(gpt|o[1-9]|codex|chatgpt|openai|davinci)/, 'openai'],
  [/deepseek/, 'deepseek'],
  [/glm|zhipu|z-ai|zai/, 'zhipu'],
  [/qwen|qwq|alibaba/, 'qwen'],
  [/mistral|mixtral|codestral|devstral|magistral/, 'mistral'],
  [/grok|xai/, 'xai'],
  [/llama|meta/, 'meta'],
  [/kimi|moonshot/, 'moonshot'],
];

// "claude-opus-5-5" -> "Opus 5.5"; "gpt-6.1-sol" -> "GPT-6.1 Sol".
function variantName(model, provider) {
  const base = model.split('/').pop().replace(/:.*$/, '');
  if (provider === 'anthropic') {
    const m = base.match(/(opus|sonnet|haiku|fable)-?([\d-]*)/);
    if (m) return cap(m[1]) + (m[2] ? ' ' + m[2].replace(/-/g, '.').replace(/\.$/, '') : '');
  }
  if (provider === 'openai') {
    return base.replace(/^gpt-/, 'GPT-').split('-').map((p, i) => (i ? cap(p) : p)).join(' ');
  }
  return base.split(/[-_]/).filter(Boolean).slice(0, 3).map(cap).join(' ');
}

const ACRONYMS = /^(glm|gpt|lfm|ai)$/;

function cap(s) {
  if (!s) return s;
  if (ACRONYMS.test(s)) return s.toUpperCase();
  return s.replace(/^deepseek/, 'DeepSeek').replace(/^[a-z]/, (c) => c.toUpperCase());
}

export function liveryFor(model) {
  const raw = (model || '').toLowerCase();
  const provider = RULES.find(([re]) => re.test(raw))?.[1] ?? 'generic';
  const base = PROVIDERS[provider];
  const variant = raw ? variantName(raw, provider) : '';
  const livery = { ...base, provider, variant, key: `${provider}|${variant}` };
  if (provider === 'google' && /gemma/.test(raw)) livery.title = 'Gemma';
  if (provider === 'generic') {
    livery.title = variant || null;
    livery.letter = (variant || '?').slice(0, 1).toUpperCase();
  }
  return livery;
}

/* ---------------------------------------------------- texture caching */

// Textures are shared between aircraft with the same livery + airframe and
// are never disposed per aircraft (they are flagged `userData.shared`).
const cache = new Map();

function cachedTexture(key, draw, w, h) {
  if (cache.has(key)) return cache.get(key);
  const c = document.createElement('canvas');
  c.width = w;
  c.height = h;
  draw(c.getContext('2d'), w, h);
  const tex = new THREE.CanvasTexture(c);
  tex.colorSpace = THREE.SRGBColorSpace;
  tex.anisotropy = 4;
  tex.userData.shared = true;
  cache.set(key, tex);
  return tex;
}

/* ------------------------------------------------------- fuselage skin */

// Layout of a fuselage, all in world units of the airframe:
//   zmin/zmax  extent along the body, circ: circumference at the widest point
//   windows:   { kind: 'row' | 'highwing', ... } (a = radians from top)
//   cockpit:   { z0, z1, a, frames?, frameColor? } windscreen centred on the top
//   titles:    { a, z, size }, nose: { a, z, size } for the variant name
//   stripeA:   cheatline angle from top; bellyA: where the belly colour starts
//   swooshZ:   z where the tail colour starts sweeping up over the spine
//   radomeZ:   optional z where a dark dielectric nose radome starts
//   antiGlare: optional { z0, z1, a, color } matte panel ahead of cockpit
//   military:  optional low-visibility roundels, panel lines and stencils
export function fuselageTexture(livery, airframe, L) {
  return cachedTexture(`fus|${airframe}|${livery.key}`, (ctx, W, H) => {
    const len = L.zmax - L.zmin;
    const C = L.circ;
    ctx.scale(W / C, H / len);
    // Draw space: X wraps around the body (C/2 = spine, C/4 = +X side,
    // 3C/4 = -X side, 0/C = belly); Y runs nose (0) -> tail (len).
    const X = (a) => ((Math.PI - a) / (2 * Math.PI)) * C;
    const Y = (z) => L.zmax - z;

    const bodyColor = L.bodyColor ?? livery.body;
    const bellyColor = L.bellyColor ?? livery.belly;

    ctx.fillStyle = bodyColor;
    ctx.fillRect(0, 0, C, len);

    // Belly.
    if (L.bellyA !== undefined) {
      ctx.fillStyle = bellyColor;
      ctx.fillRect(0, 0, X(L.bellyA), len);
      ctx.fillRect(X(-L.bellyA), 0, C - X(-L.bellyA), len);
    }

    // Tail colour sweeping up over the spine toward the fin.
    if (L.swooshZ !== undefined) {
      // Reach (angle from the spine) grows from 0 at swooshZ to the belly
      // line at the tail; traced down the +X side and back up the -X side.
      ctx.fillStyle = livery.swoosh;
      const steps = 48;
      const pts = [];
      for (let i = 0; i <= steps; i++) {
        const t = i / steps;
        const z = L.swooshZ - (L.swooshZ - L.zmin) * t;
        pts.push([L.bellyA * (1 - (1 - t) ** 2.2), Y(z)]);
      }
      ctx.beginPath();
      pts.forEach(([a, y]) => ctx.lineTo(X(a), y));
      ctx.lineTo(X(L.bellyA), len + 1);
      ctx.lineTo(X(-L.bellyA), len + 1);
      for (let i = pts.length - 1; i >= 0; i--) ctx.lineTo(X(-pts[i][0]), pts[i][1]);
      ctx.closePath();
      ctx.fill();
    }

    // Cheatline stripes along both sides (stacked downward).
    if (L.stripeA !== undefined) {
      const sw = L.stripeW ?? 0.12;
      livery.stripes.forEach((col, i) => {
        ctx.fillStyle = col;
        const a = L.stripeA + i * sw;
        const y0 = Y(L.stripeZ1 ?? L.zmax - 0.15 * len);
        const y1 = Y(L.swooshZ ?? L.zmin + 0.1 * len);
        for (const side of [1, -1]) {
          const xa = X(side * a);
          const xb = X(side * (a + sw * 0.85));
          ctx.fillRect(Math.min(xa, xb), y0, Math.abs(xb - xa), y1 - y0);
        }
      });
    }

    // Darker grey dielectric nose radome cap (military transport / tanker).
    if (L.radomeZ !== undefined) {
      ctx.fillStyle = L.radomeColor ?? TANKER_GREY.radome;
      ctx.fillRect(0, 0, C, Y(L.radomeZ));
      ctx.fillStyle = TANKER_GREY.panel;
      ctx.fillRect(0, Y(L.radomeZ), C, 0.08);
    }

    // Matte anti-glare strip directly ahead of the cockpit windscreen.
    if (L.antiGlare) {
      const { z0, z1, a, color = TANKER_GREY.antiGlare } = L.antiGlare;
      ctx.fillStyle = color;
      ctx.beginPath();
      ctx.moveTo(X(a), Y(z0));
      ctx.lineTo(X(a * 0.7), Y(z1));
      ctx.lineTo(X(-a * 0.7), Y(z1));
      ctx.lineTo(X(-a), Y(z0));
      ctx.closePath();
      ctx.fill();
    }

    // Low-visibility military panel seams, fuselage rings, roundels & walkways.
    if (L.military) {
      const M = L.military;
      ctx.strokeStyle = TANKER_GREY.panel;
      ctx.lineWidth = 0.06;
      // Subtle fuselage frame seams (spaced clear of the flank roundel).
      for (const z of M.frameZs ?? [-11.0, -2.0, 3.4, 8.2, 11.6]) {
        ctx.beginPath();
        ctx.moveTo(0, Y(z));
        ctx.lineTo(C, Y(z));
        ctx.stroke();
      }
      // Subtle lower-flank tactical separation pinstripe.
      if (L.bellyA !== undefined) {
        for (const side of [1, -1]) {
          const x = X(side * L.bellyA);
          ctx.beginPath();
          ctx.moveTo(x, Y(L.radomeZ ?? L.zmax - 2));
          ctx.lineTo(x, Y(L.zmin + 2));
          ctx.stroke();
        }
      }
      // Low-visibility dorsal walkway lines on the upper spine.
      ctx.strokeStyle = 'rgba(42, 49, 56, 0.34)';
      ctx.lineWidth = 0.07;
      for (const side of [1, -1]) {
        const x = X(side * 0.34);
        ctx.beginPath();
        ctx.moveTo(x, Y(11.0));
        ctx.lineTo(x, Y(-8.0));
        ctx.stroke();
      }
      // Low-visibility military roundels with horizontal tactical bars on both flanks.
      if (M.roundelZ !== undefined) {
        const r = M.roundelR ?? 0.94;
        const barW = r * 1.75;
        const barH = r * 0.44;
        for (const side of [1, -1]) {
          const cx = X(side * M.roundelA);
          const cy = Y(M.roundelZ);
          ctx.fillStyle = TANKER_GREY.roundelOuter;
          ctx.fillRect(cx - barH / 2, cy - barW, barH, barW * 2);
          ctx.fillStyle = TANKER_GREY.roundelMid;
          ctx.fillRect(cx - barH * 0.24, cy - barW * 0.88, barH * 0.48, barW * 1.76);
          ctx.beginPath();
          ctx.arc(cx, cy, r, 0, Math.PI * 2);
          ctx.fillStyle = TANKER_GREY.roundelOuter;
          ctx.fill();
          ctx.beginPath();
          ctx.arc(cx, cy, r * 0.66, 0, Math.PI * 2);
          ctx.fillStyle = TANKER_GREY.roundelMid;
          ctx.fill();
          ctx.beginPath();
          ctx.arc(cx, cy, r * 0.36, 0, Math.PI * 2);
          ctx.fillStyle = TANKER_GREY.roundelCore;
          ctx.fill();
        }
      }
    }

    // Subtle cabin door outlines (widebody detail).
    if (L.doors) {
      ctx.strokeStyle = 'rgba(20, 28, 36, 0.32)';
      ctx.lineWidth = 0.055;
      for (const side of [1, -1]) {
        const xc = X(side * L.doors.a);
        for (const z of L.doors.zs) {
          roundRect(ctx, xc - L.doors.h / 2, Y(z) - L.doors.w / 2, L.doors.h, L.doors.w, 0.1);
          ctx.stroke();
        }
      }
    }

    // Windows (supports a single window layout or an array of rows, e.g. 747 upper + main decks).
    ctx.fillStyle = '#1b2a3a';
    const windowSets = Array.isArray(L.windows) ? L.windows : (L.windows ? [L.windows] : []);
    for (const wnd of windowSets) {
      if (wnd?.kind === 'row') {
        for (const side of [1, -1]) {
          const xc = X(side * wnd.a);
          for (let z = wnd.z1; z >= wnd.z0; z -= wnd.pitch) {
            roundRect(ctx, xc - wnd.h / 2, Y(z) - wnd.w / 2, wnd.h, wnd.w, Math.min(wnd.h, wnd.w) * 0.45);
            ctx.fill();
          }
        }
      } else if (wnd?.kind === 'highwing') {
        // Light high-wing cabin: windscreen wrapping round to a raked
        // A-pillar, door window, tapering rear side window and a skylight
        // behind the wing. Points are [angle from top, z].
        const poly = (pts) => {
          ctx.beginPath();
          pts.forEach(([a, z]) => ctx.lineTo(X(a), Y(z)));
          ctx.closePath();
          ctx.fill();
          ctx.stroke(); // rounds the corners a touch
        };
        ctx.strokeStyle = ctx.fillStyle;
        ctx.lineJoin = 'round';
        ctx.lineWidth = wnd.post * 0.6;
        const { top, sill, post, front, pillarTop, pillarBottom, doorEnd, rearEnd, rearTaper } = wnd;
        for (const s of [1, -1]) {
          poly([[s * top, pillarTop - post], [s * top, doorEnd + post / 2],
            [s * sill, doorEnd + post / 2], [s * sill, pillarBottom - post]]);
          poly([[s * top, doorEnd - post / 2], [s * top, rearEnd],
            [s * (sill - 0.08), rearEnd + rearTaper], [s * sill, doorEnd - post / 2]]);
        }
        const wsA = top * 0.9;
        poly([[-wsA, front], [wsA, front], [sill - 0.06, pillarBottom + post],
          [top, pillarTop + post], [-top, pillarTop + post], [-(sill - 0.06), pillarBottom + post]]);
        const [k0, k1, ka] = wnd.skylight;
        poly([[-ka, k0], [ka, k0], [ka * 0.75, k1], [-ka * 0.75, k1]]);
        // Windscreen centre strip.
        ctx.strokeStyle = bodyColor;
        ctx.lineWidth = post * 0.7;
        ctx.beginPath();
        ctx.moveTo(X(0), Y(pillarTop + post));
        ctx.lineTo(X(0), Y(front));
        ctx.stroke();
      }
    }
    if (L.cockpit) {
      const { z0, z1, a, frames = [-0.33, 0, 0.33], frameColor = bodyColor } = L.cockpit;
      ctx.beginPath();
      ctx.moveTo(X(a), Y(z0));
      ctx.lineTo(X(a * 0.8), Y(z1));
      ctx.lineTo(X(-a * 0.8), Y(z1));
      ctx.lineTo(X(-a), Y(z0));
      ctx.closePath();
      ctx.fill();
      // Window frames.
      ctx.strokeStyle = frameColor;
      ctx.lineWidth = 0.055;
      for (const f of frames) {
        ctx.beginPath();
        ctx.moveTo(X(a * f * 1.5), Y(z0));
        ctx.lineTo(X(a * f * 1.2), Y(z1));
        ctx.stroke();
      }
    }

    // Titles along both sides, upright and reading nose-to-tail from either side.
    // Text is set in pixel space (tiny fractional font sizes under a scaled
    // transform render unreliably), then stretched so world proportions hold.
    const kX = W / C;
    const kY = H / len;
    const sideText = (text, a, z, size, font, color, spacing = 0) => {
      if (!text) return;
      const px = size * kX;
      ctx.save();
      ctx.setTransform(1, 0, 0, 1, 0, 0);
      ctx.fillStyle = color;
      ctx.font = font.replace('1px', `${px}px`);
      ctx.textAlign = 'center';
      ctx.textBaseline = 'middle';
      if ('letterSpacing' in ctx) ctx.letterSpacing = `${spacing * px}px`;
      for (const side of [1, -1]) {
        ctx.save();
        ctx.translate(X(side * a) * kX, Y(z) * kY);
        ctx.rotate(side * Math.PI / 2);
        ctx.scale(kY / kX, 1);
        ctx.fillText(text, 0, 0);
        ctx.restore();
      }
      ctx.restore();
    };
    if (L.titles) {
      sideText(livery.title, L.titles.a, L.titles.z, L.titles.size, livery.titleFont, livery.titleColor);
      if (livery.operator && L.operator) {
        sideText(livery.operator, L.operator.a, L.operator.z, L.operator.size,
          `600 1px ${SANS}`, livery.titleColor, 0.18);
      }
    }
    if (L.nose && livery.variant) {
      sideText(livery.variant, L.nose.a, L.nose.z, L.nose.size, `600 1px ${SANS}`, livery.titleColor);
    }
    // Low-visibility military stencil markings for tankers (small tactical text).
    if (L.military) {
      const M = L.military;
      const stencilFont = `700 1px "IBM Plex Mono", ui-monospace, ${SANS}`;
      const labTag = (livery.operator || livery.title || livery.provider || 'AIR').toUpperCase();
      if (M.stencilZ !== undefined) {
        sideText(`AIR REFUEL · ${labTag}`, M.stencilA, M.stencilZ, M.stencilSize ?? 0.44,
          stencilFont, TANKER_GREY.stencil, 0.14);
      }
      if (M.forwardStencilZ !== undefined) {
        sideText('KC-TANKER // NO STEP', M.forwardStencilA, M.forwardStencilZ, M.forwardStencilSize ?? 0.25,
          stencilFont, TANKER_GREY.stencil, 0.1);
      }
    }
  }, L.texW ?? 512, L.texH ?? 1024);
}

/* ------------------------------------------------------ tail fin skin */

// Vertical gradient for fins (fin UV v runs root -> tip).
export function finTexture(livery) {
  return cachedTexture(`fin|${livery.key}`, (ctx, W, H) => {
    const cols = livery.tail;
    if (cols.length === 1) {
      ctx.fillStyle = cols[0];
    } else {
      const g = ctx.createLinearGradient(0, H, W, 0);
      cols.forEach((c, i) => g.addColorStop(i / (cols.length - 1), c));
      ctx.fillStyle = g;
    }
    ctx.fillRect(0, 0, W, H);
    // Rudder hinge (u is the chord fraction, see surfaceGeo).
    ctx.fillStyle = 'rgba(0, 0, 0, 0.05)';
    ctx.fillRect(W * 0.7, 0, W * 0.3, H);
    ctx.fillStyle = 'rgba(0, 0, 0, 0.28)';
    ctx.fillRect(W * 0.7, 0, 1.5, H);
  }, 128, 256);
}

/* ------------------------------------------------ wing and tail skins */

// Flying-surface skin over a base colour. u runs leading -> trailing edge
// (chord fraction) and v root -> tip (canvas top is the tip). Wings get a
// bare-metal leading edge, spoiler panels, flaps inboard and ailerons
// outboard; tails get a single elevator hinge.
export function surfaceTexture(livery, base, kind = 'wing') {
  return cachedTexture(`surf|${kind}|${base}|${livery.key}`, (ctx, W, H) => {
    const U = (u) => u * W;
    const V = (v) => (1 - v) * H;
    const isWing = kind === 'wing' || kind === 'tanker-wing';
    const isMilitary = kind === 'tanker-wing' || kind === 'tanker-tail';
    ctx.fillStyle = base;
    ctx.fillRect(0, 0, W, H);
    const line = isMilitary ? 'rgba(20, 26, 32, 0.32)' : 'rgba(0, 0, 0, 0.26)';
    const hingeU = isWing ? 0.74 : 0.7;

    // Leading edge: subtle de-icer / erosion strip (matte dark grey on military tankers).
    ctx.fillStyle = isMilitary ? 'rgba(42, 48, 56, 0.28)' : 'rgba(200, 208, 216, 0.32)';
    ctx.fillRect(0, 0, U(isWing ? 0.08 : 0.06), H);

    // Control surfaces, a shade darker behind the hinge line.
    ctx.fillStyle = isMilitary ? 'rgba(0, 0, 0, 0.1)' : 'rgba(0, 0, 0, 0.06)';
    ctx.fillRect(U(hingeU), 0, W - U(hingeU), H);
    ctx.fillStyle = line;
    ctx.fillRect(U(hingeU), 0, 2, H);

    if (isWing) {
      // Flap / aileron splits.
      for (const v of [0.08, 0.38, 0.66, 0.94]) ctx.fillRect(U(hingeU), V(v), W - U(hingeU), 2);
      // Spoiler panels ahead of the flaps.
      ctx.strokeStyle = isMilitary ? 'rgba(20, 26, 32, 0.24)' : 'rgba(0, 0, 0, 0.18)';
      ctx.lineWidth = 1.5;
      for (let i = 0; i < 5; i++) {
        const v0 = 0.1 + i * 0.11;
        ctx.strokeRect(U(0.6), V(v0 + 0.1), U(hingeU) - U(0.6), V(v0) - V(v0 + 0.1));
      }
      // Front spar / slat line.
      ctx.fillStyle = isMilitary ? 'rgba(20, 26, 32, 0.2)' : 'rgba(0, 0, 0, 0.12)';
      ctx.fillRect(U(0.14), 0, 1.5, H);

      if (isMilitary) {
        // Low-visibility tactical wing roundel & walkway boundary line.
        ctx.strokeStyle = 'rgba(42, 49, 56, 0.34)';
        ctx.lineWidth = 2;
        ctx.strokeRect(U(0.24), V(0.54), U(0.32), V(0.08) - V(0.54));
        const rx = U(0.42);
        const ry = V(0.74);
        ctx.beginPath();
        ctx.arc(rx, ry, 20, 0, Math.PI * 2);
        ctx.fillStyle = TANKER_GREY.roundelOuter;
        ctx.fill();
        ctx.beginPath();
        ctx.arc(rx, ry, 13.5, 0, Math.PI * 2);
        ctx.fillStyle = TANKER_GREY.roundelMid;
        ctx.fill();
        ctx.beginPath();
        ctx.arc(rx, ry, 7, 0, Math.PI * 2);
        ctx.fillStyle = TANKER_GREY.roundelCore;
        ctx.fill();
      }
    }
  }, 256, 256);
}

/* ------------------------------------------------------------- logos */

export function logoTexture(livery) {
  return cachedTexture(`logo|${livery.key}`, (ctx, W) => {
    ctx.translate(W / 2, W / 2);
    ctx.scale(W / 2, W / 2); // draw in [-1, 1]
    ctx.fillStyle = livery.logoColor;
    ctx.strokeStyle = livery.logoColor;
    (LOGOS[livery.logo] ?? LOGOS.letter)(ctx, livery, W);
  }, 256, 256);
}

const LOGOS = {
  // Radiating spark of uneven, tapered rays.
  claudeSpark(ctx) {
    const rays = 12;
    for (let i = 0; i < rays; i++) {
      const ang = (i / rays) * Math.PI * 2 + 0.13;
      const len = 0.62 + 0.28 * ((i * 7) % 5) / 4;
      ctx.save();
      ctx.rotate(ang);
      ctx.beginPath();
      ctx.moveTo(-0.07, 0);
      ctx.lineTo(-0.045, -len);
      ctx.quadraticCurveTo(0, -len - 0.06, 0.045, -len);
      ctx.lineTo(0.07, 0);
      ctx.closePath();
      ctx.fill();
      ctx.restore();
    }
    ctx.beginPath();
    ctx.arc(0, 0, 0.16, 0, Math.PI * 2);
    ctx.fill();
  },
  // Six interlocking rounded links forming a hexagonal blossom.
  blossom(ctx) {
    ctx.lineWidth = 0.11;
    ctx.lineJoin = 'round';
    for (let i = 0; i < 6; i++) {
      ctx.save();
      ctx.rotate((i * Math.PI) / 3);
      roundRect(ctx, -0.2, -0.82, 0.4, 0.95, 0.2);
      ctx.stroke();
      ctx.restore();
    }
  },
  // Four-point sparkle with concave sides.
  sparkle(ctx) {
    ctx.beginPath();
    ctx.moveTo(0, -0.95);
    ctx.quadraticCurveTo(0.08, -0.08, 0.95, 0);
    ctx.quadraticCurveTo(0.08, 0.08, 0, 0.95);
    ctx.quadraticCurveTo(-0.08, 0.08, -0.95, 0);
    ctx.quadraticCurveTo(-0.08, -0.08, 0, -0.95);
    ctx.fill();
  },
  // Stylised whale.
  whale(ctx) {
    ctx.beginPath();
    ctx.ellipse(-0.05, 0.08, 0.68, 0.4, -0.1, 0, Math.PI * 2);
    ctx.fill();
    ctx.beginPath();
    ctx.moveTo(0.5, 0.0);
    ctx.quadraticCurveTo(0.75, -0.15, 0.82, -0.52);
    ctx.lineTo(0.95, -0.3);
    ctx.quadraticCurveTo(0.85, -0.1, 0.62, 0.18);
    ctx.closePath();
    ctx.fill();
    ctx.globalCompositeOperation = 'destination-out';
    ctx.beginPath();
    ctx.arc(-0.42, -0.02, 0.07, 0, Math.PI * 2);
    ctx.fill();
    ctx.globalCompositeOperation = 'source-over';
  },
  zed(ctx) {
    ctx.beginPath();
    ctx.moveTo(-0.62, -0.7);
    ctx.lineTo(0.62, -0.7);
    ctx.lineTo(0.62, -0.42);
    ctx.lineTo(-0.2, 0.44);
    ctx.lineTo(0.62, 0.44);
    ctx.lineTo(0.62, 0.7);
    ctx.lineTo(-0.62, 0.7);
    ctx.lineTo(-0.62, 0.42);
    ctx.lineTo(0.2, -0.44);
    ctx.lineTo(-0.62, -0.44);
    ctx.closePath();
    ctx.fill();
  },
  // Hexagon with a tail: a geometric "Q".
  hexQ(ctx) {
    ctx.lineWidth = 0.17;
    ctx.lineJoin = 'round';
    ctx.beginPath();
    for (let i = 0; i < 6; i++) {
      const a = (i / 6) * Math.PI * 2 + Math.PI / 6;
      ctx.lineTo(Math.cos(a) * 0.68, Math.sin(a) * 0.68);
    }
    ctx.closePath();
    ctx.stroke();
    ctx.lineCap = 'round';
    ctx.beginPath();
    ctx.moveTo(0.18, 0.3);
    ctx.lineTo(0.68, 0.82);
    ctx.stroke();
  },
  // Block-pixel "M" in a sunset gradient.
  pixelM(ctx) {
    const rows = ['#ffd800', '#ffaf00', '#ff8205', '#fa500f', '#e10500'];
    const grid = ['X...X', 'XX.XX', 'X.X.X', 'X...X', 'X...X'];
    const s = 0.34;
    grid.forEach((row, r) => {
      ctx.fillStyle = rows[r];
      [...row].forEach((ch, c) => {
        if (ch === 'X') ctx.fillRect(-0.85 + c * s, -0.85 + r * s, s * 0.98, s * 0.98);
      });
    });
  },
  slash(ctx) {
    ctx.lineWidth = 0.2;
    ctx.lineCap = 'butt';
    ctx.beginPath();
    ctx.moveTo(-0.55, 0.75);
    ctx.lineTo(0.55, -0.75);
    ctx.stroke();
    ctx.lineWidth = 0.1;
    ctx.beginPath();
    ctx.moveTo(-0.6, -0.55);
    ctx.lineTo(0.05, 0.2);
    ctx.stroke();
  },
  loop(ctx) {
    ctx.lineWidth = 0.17;
    ctx.beginPath();
    for (let i = 0; i <= 80; i++) {
      const t = (i / 80) * Math.PI * 2;
      const d = 1 + Math.sin(t) ** 2;
      ctx.lineTo((0.8 * Math.cos(t)) / d, (0.8 * Math.sin(t) * Math.cos(t)) / d);
    }
    ctx.closePath();
    ctx.stroke();
  },
  letter(ctx, livery, W) {
    ctx.setTransform(1, 0, 0, 1, W / 2, W / 2);
    ctx.font = `800 ${W * 0.75}px ${SANS}`;
    ctx.textAlign = 'center';
    ctx.textBaseline = 'middle';
    ctx.fillText(livery.letter ?? '?', 0, W * 0.04);
  },
};

function roundRect(ctx, x, y, w, h, r) {
  ctx.beginPath();
  ctx.moveTo(x + r, y);
  ctx.arcTo(x + w, y, x + w, y + h, r);
  ctx.arcTo(x + w, y + h, x, y + h, r);
  ctx.arcTo(x, y + h, x, y, r);
  ctx.arcTo(x, y, x + w, y, r);
  ctx.closePath();
}
