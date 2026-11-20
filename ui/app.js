// Streamee — interface du pad. Tout l'audio est côté Rust ; ici on affiche et on envoie des commandes.
'use strict';

const invoke = window.__TAURI__.core.invoke;
const $ = (id) => document.getElementById(id);

const COLORS = ['#2e86de', '#1f9d55', '#d98a1c', '#d64541', '#8e44ad', '#16a085', '#c0392b', '#f1c40f', '#e84393', '#636e72'];

let config = { pads: [], settings: {} };
let durations = {};
let loadErrors = {};
let editing = false;
let playing = new Map();  // id -> progression 0..1
const padEls = new Map(); // id -> élément

// ------------------------------------------------------------------ Utilitaires

let toastTimer;
function toast(msg) {
  if (!msg) return;
  const t = $('toast');
  t.textContent = msg;
  t.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => (t.hidden = true), 5000);
}

function reportErrors(list) {
  if (list && list.length) toast(list.join('\n'));
}

async function call(cmd, args) {
  try {
    return await invoke(cmd, args);
  } catch (e) {
    toast(String(e));
    throw e;
  }
}

function debounce(fn, ms) {
  let t;
  return (...a) => { clearTimeout(t); t = setTimeout(() => fn(...a), ms); };
}

// Raccourcis : on stocke le format du plugin (« Ctrl+Alt+KeyA »), on affiche « Ctrl+Alt+A ».
function prettyKey(h) {
  if (!h) return '';
  return h.split('+').map((p) => p
    .replace(/^Key([A-Z])$/, '$1')
    .replace(/^Digit(\d)$/, '$1')
    .replace(/^Numpad(.+)$/, 'Pavé $1')
    .replace(/^Arrow(.+)$/, '$1')
    .replace('Backspace', 'Retour')
    .replace('Super', 'Win')).join('+');
}

const MODIFIER_CODES = ['ControlLeft', 'ControlRight', 'AltLeft', 'AltRight', 'ShiftLeft', 'ShiftRight', 'MetaLeft', 'MetaRight'];

/** Écoute la prochaine combinaison de touches sur un bouton. */
function captureHotkey(btn, current, onDone) {
  btn.classList.add('listening');
  btn.textContent = 'Appuie sur la combinaison… (Échap : annuler)';
  const handler = (e) => {
    e.preventDefault();
    e.stopPropagation();
    if (e.code === 'Escape') return finish(current);
    if (MODIFIER_CODES.includes(e.code)) return; // on attend la vraie touche
    const mods = [];
    if (e.ctrlKey) mods.push('Ctrl');
    if (e.altKey) mods.push('Alt');
    if (e.shiftKey) mods.push('Shift');
    if (e.metaKey) mods.push('Super');
    const value = [...mods, e.code].join('+');
    if (!mods.length && /^(Key|Digit)/.test(e.code)) {
      toast('Sans Ctrl/Alt/Maj, cette touche ne marcherait plus pour écrire ailleurs : ajoute un modificateur.');
      return;
    }
    finish(value);
  };
  function finish(value) {
    window.removeEventListener('keydown', handler, true);
    btn.classList.remove('listening');
    onDone(value);
  }
  window.addEventListener('keydown', handler, true);
}

function showHotkey(btn, value) {
  btn.textContent = value ? prettyKey(value) : 'Aucun — cliquer pour en choisir un';
}

// ------------------------------------------------------------------ Grille

function render() {
  const grid = $('grid');
  document.documentElement.style.setProperty('--cols', config.settings.columns || 4);
  grid.textContent = '';
  padEls.clear();

  if (!config.pads.length) {
    const empty = document.createElement('div');
    empty.className = 'empty';
    empty.innerHTML = 'Aucun son pour l\'instant.<br>Glisse des fichiers audio dans la fenêtre ou clique ci-dessous.';
    grid.append(empty);
  }

  for (const pad of config.pads) {
    const el = document.createElement('button');
    el.className = 'pad';
    el.style.setProperty('--c', pad.color);
    if (loadErrors[pad.id]) {
      el.classList.add('broken');
      el.title = loadErrors[pad.id];
    }
    const progress = document.createElement('div');
    progress.className = 'progress';
    const name = document.createElement('div');
    name.className = 'name';
    name.textContent = pad.name;
    const meta = document.createElement('div');
    meta.className = 'meta';
    if (pad.hotkey) {
      const k = document.createElement('span');
      k.className = 'key';
      k.textContent = prettyKey(pad.hotkey);
      meta.append(k);
    }
    const info = [];
    if (durations[pad.id] != null) info.push(formatDuration(durations[pad.id]));
    if (pad.looped) info.push('⟲');
    if (pad.mode === 'toggle') info.push('⏯');
    if (pad.mode === 'overlap') info.push('⧉');
    const span = document.createElement('span');
    span.textContent = info.join(' ');
    meta.append(span);
    el.append(progress, name, meta);

    el.addEventListener('click', () => (editing ? openPad(pad.id) : invoke('play_pad', { id: pad.id })));
    el.addEventListener('contextmenu', (e) => { e.preventDefault(); openPad(pad.id); });
    grid.append(el);
    padEls.set(pad.id, { el, progress });
  }

  const add = document.createElement('button');
  add.className = 'add';
  add.textContent = '+ Ajouter des sons';
  add.addEventListener('click', pickSounds);
  grid.append(add);
  paintPlaying();
}

function formatDuration(s) {
  if (s < 10) return s.toFixed(1) + ' s';
  const m = Math.floor(s / 60);
  const r = Math.round(s % 60);
  return m ? `${m}:${String(r).padStart(2, '0')}` : `${r} s`;
}

function paintPlaying() {
  for (const [id, { el, progress }] of padEls) {
    const p = playing.get(id);
    el.classList.toggle('playing', p != null);
    progress.style.width = p != null ? `${(p * 100).toFixed(1)}%` : '0';
  }
}

// ------------------------------------------------------------------ Chargement

async function refresh(showHotkeyErrors = false) {
  const s = await invoke('get_state');
  config = s.config;
  durations = s.durations;
  loadErrors = s.loadErrors;
  render();
  syncBar();
  if (showHotkeyErrors) reportErrors(s.hotkeyErrors);
}

async function pickSounds() {
  const r = await call('pick_sounds');
  afterAdd(r);
}

async function afterAdd(r) {
  reportErrors(r.errors);
  if (r.added) await refresh();
}

// ------------------------------------------------------------------ Barre du haut

function syncBar() {
  const s = config.settings;
  $('injectVol').value = Math.round(s.injectVolume * 100);
  $('monitorVol').value = Math.round(s.monitorVolume * 100);
  $('monitorToggle').setAttribute('aria-pressed', String(s.monitorEnabled));
}

const saveSettings = debounce(async () => {
  const errors = await call('set_settings', { settings: config.settings });
  reportErrors(errors);
}, 150);

$('injectVol').addEventListener('input', (e) => { config.settings.injectVolume = e.target.value / 100; saveSettings(); });
$('monitorVol').addEventListener('input', (e) => { config.settings.monitorVolume = e.target.value / 100; saveSettings(); });
$('monitorToggle').addEventListener('click', (e) => {
  e.preventDefault();
  config.settings.monitorEnabled = !config.settings.monitorEnabled;
  syncBar();
  saveSettings();
});
$('stopBtn').addEventListener('click', () => invoke('stop_all'));
$('editBtn').addEventListener('click', () => {
  editing = !editing;
  document.body.classList.toggle('editing', editing);
  $('editBtn').setAttribute('aria-pressed', String(editing));
});
$('settingsBtn').addEventListener('click', openSettings);
$('mic').addEventListener('click', openSettings);

window.addEventListener('keydown', (e) => {
  if (document.querySelector('dialog[open]')) return;
  if (e.code === 'Escape') invoke('stop_all');
});

// ------------------------------------------------------------------ État du micro

let lastApo = null;
function showMic(st) {
  const mic = $('mic');
  const text = $('micText');
  const name = st.apoEndpoint;
  let cls, label;
  if (!name) {
    cls = 'off'; label = 'Micro non configuré';
  } else if (!st.inject.ready) {
    cls = 'off'; label = `${name} · à réinstaller`;
  } else if (st.inject.active) {
    cls = 'live'; label = `${name} · en direct`;
  } else {
    cls = 'wait'; label = `${name} · en veille`;
  }
  mic.className = `mic mic--${cls}`;
  text.textContent = label;
  mic.title = {
    off: 'Les sons ne partent pas encore dans le micro. Clique pour configurer.',
    wait: 'Prêt : les sons partiront dans le micro dès qu\'un logiciel l\'utilise (jeu, Discord, OBS…).',
    live: `Les sons partent dans le micro (${st.inject.rate} Hz, ${st.inject.channels} canal/canaux).`,
  }[cls];
  lastApo = st;
}

async function poll() {
  try {
    const st = await invoke('status');
    playing = new Map(st.playing);
    paintPlaying();
    showMic(st);
    if ($('setDlg').open) showApoState();
  } catch (_) { /* fenêtre en cours de fermeture */ }
  setTimeout(poll, playing.size ? 100 : 400);
}

// ------------------------------------------------------------------ Réglages d'un pad

let editPad = null;

function openPad(id) {
  const pad = config.pads.find((p) => p.id === id);
  if (!pad) return;
  editPad = { ...pad };
  $('padName').value = pad.name;
  $('padVol').value = Math.round(pad.volume * 100);
  $('padVolVal').textContent = `${$('padVol').value} %`;
  $('padMode').value = pad.mode;
  $('padLoop').checked = pad.looped;
  showHotkey($('padHotkey'), pad.hotkey);
  const sw = $('padColors');
  sw.textContent = '';
  for (const c of COLORS) {
    const b = document.createElement('button');
    b.type = 'button';
    b.style.setProperty('--c', c);
    b.setAttribute('role', 'radio');
    b.setAttribute('aria-checked', String(c === editPad.color));
    b.addEventListener('click', () => {
      editPad.color = c;
      sw.querySelectorAll('button').forEach((x) => x.setAttribute('aria-checked', String(x === b)));
    });
    sw.append(b);
  }
  $('padDlg').showModal();
}

$('padVol').addEventListener('input', (e) => { $('padVolVal').textContent = `${e.target.value} %`; });
$('padHotkey').addEventListener('click', () => {
  captureHotkey($('padHotkey'), editPad.hotkey, (v) => { editPad.hotkey = v; showHotkey($('padHotkey'), v); });
});
$('padHotkeyClear').addEventListener('click', () => { editPad.hotkey = null; showHotkey($('padHotkey'), null); });

$('padDlg').addEventListener('close', async () => {
  if ($('padDlg').returnValue !== 'save' || !editPad) return;
  editPad.name = $('padName').value.trim() || editPad.name;
  editPad.volume = $('padVol').value / 100;
  editPad.mode = $('padMode').value;
  editPad.looped = $('padLoop').checked;
  const errors = await call('update_pad', { pad: editPad });
  reportErrors(errors);
  await refresh();
});

async function movePad(delta) {
  const idx = config.pads.findIndex((p) => p.id === editPad.id);
  const to = idx + delta;
  if (idx < 0 || to < 0 || to >= config.pads.length) return;
  await call('move_pad', { id: editPad.id, to });
  await refresh();
}
$('padLeft').addEventListener('click', () => movePad(-1));
$('padRight').addEventListener('click', () => movePad(1));

$('padDelete').addEventListener('click', async () => {
  if (!confirm(`Supprimer « ${editPad.name} » ?`)) return;
  await call('delete_pad', { id: editPad.id });
  editPad = null;
  $('padDlg').close('deleted');
  await refresh();
});

// ------------------------------------------------------------------ Réglages généraux

let editSettings = null;

async function openSettings() {
  const s = await invoke('get_state');
  editSettings = { ...s.config.settings };
  const sel = $('monitorDevice');
  sel.textContent = '';
  const def = new Option('Sortie par défaut de Windows', '');
  sel.append(def);
  for (const d of s.devices) sel.append(new Option(d, d));
  if (editSettings.monitorDevice && !s.devices.includes(editSettings.monitorDevice)) {
    sel.append(new Option(`${editSettings.monitorDevice} (débranché)`, editSettings.monitorDevice));
  }
  sel.value = editSettings.monitorDevice || '';
  showHotkey($('stopHotkey'), editSettings.stopAllHotkey);
  $('columns').value = editSettings.columns;
  showApoState();
  $('setDlg').showModal();
}

function showApoState() {
  const st = lastApo;
  const el = $('apoState');
  if (!st || !st.apoEndpoint) {
    el.textContent = 'Pas encore installé : les sons ne sortent que dans ton casque.';
  } else if (!st.inject.ready) {
    el.textContent = `Installé sur « ${st.apoEndpoint} » mais le tampon partagé est introuvable : relance l'installation.`;
  } else if (st.inject.active) {
    el.textContent = `Installé sur « ${st.apoEndpoint} » : en direct (${st.inject.rate} Hz).`;
  } else {
    el.textContent = `Installé sur « ${st.apoEndpoint} ». En veille tant qu'aucun logiciel n'utilise ce micro.`;
  }
}

$('stopHotkey').addEventListener('click', () => {
  captureHotkey($('stopHotkey'), editSettings.stopAllHotkey, (v) => { editSettings.stopAllHotkey = v; showHotkey($('stopHotkey'), v); });
});
$('stopHotkeyClear').addEventListener('click', () => { editSettings.stopAllHotkey = null; showHotkey($('stopHotkey'), null); });
$('apoInstall').addEventListener('click', () => call('run_apo_script', { uninstall: false }));
$('apoUninstall').addEventListener('click', () => {
  if (confirm('Retirer Streamee de tous les micros et remettre leurs réglages d\'origine ?')) {
    call('run_apo_script', { uninstall: true });
  }
});

$('setDlg').addEventListener('close', async () => {
  if ($('setDlg').returnValue !== 'save') return;
  editSettings.monitorDevice = $('monitorDevice').value || null;
  editSettings.columns = Math.min(10, Math.max(2, parseInt($('columns').value, 10) || 4));
  // Les volumes ont pu bouger dans la barre pendant que la fenêtre était ouverte.
  editSettings.injectVolume = config.settings.injectVolume;
  editSettings.monitorVolume = config.settings.monitorVolume;
  editSettings.monitorEnabled = config.settings.monitorEnabled;
  config.settings = editSettings;
  const errors = await call('set_settings', { settings: editSettings });
  reportErrors(errors);
  render();
});

// ------------------------------------------------------------------ Glisser-déposer de fichiers

try {
  window.__TAURI__.webview.getCurrentWebview().onDragDropEvent(async (e) => {
    const t = e.payload.type;
    $('dropHint').hidden = !(t === 'enter' || t === 'over');
    if (t === 'drop' && e.payload.paths.length) {
      afterAdd(await call('add_sounds', { paths: e.payload.paths }));
    }
  });
} catch (_) { /* hors Tauri */ }

// Les sons chargés en arrière-plan au démarrage : on rafraîchit les durées quelques fois.
refresh(true).then(() => {
  poll();
  let n = 0;
  const t = setInterval(() => {
    if (++n > 5 || config.pads.every((p) => durations[p.id] != null || loadErrors[p.id])) clearInterval(t);
    else refresh();
  }, 1000);
});
