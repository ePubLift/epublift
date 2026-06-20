// Front-end logic for epublift-web. Served at /app.js as a same-origin script so
// the Content-Security-Policy can use `script-src 'self'` (no inline scripts).
// Safe translation lookup — falls back to the key if i18n.js hasn't loaded.
const T = (key) => (window.i18n && window.i18n.t) ? window.i18n.t(key) : key;
// Fill {name} placeholders in a translated template.
const fill = (key, vars) => T(key).replace(/\{(\w+)\}/g, (_, k) => (k in vars ? vars[k] : '{' + k + '}'));

const q = document.getElementById('quality');
const qval = document.getElementById('qval');
const setFill = () => {
  qval.textContent = q.value;
  q.style.setProperty('--p', ((q.value - 1) / 99 * 100) + '%');
};
q.addEventListener('input', setFill); setFill();

const drop = document.getElementById('drop');
const file = document.getElementById('file');
const chip = document.getElementById('chip');
const chipname = document.getElementById('chipname');
const result = document.getElementById('result');
const outname = document.getElementById('outname');
const ascii = document.getElementById('ascii');
const kepub = document.getElementById('kepub');
const keepImages = document.getElementById('keep_images');
const modernize = document.getElementById('modernize');
const go = document.getElementById('go');

const dropTitle = document.getElementById('dropTitle');
const optsH = document.getElementById('optsH');
const ctaLabel = document.getElementById('ctaLabel');
const resReady = document.getElementById('resReady');
const resultSub = document.getElementById('resultSub');
const modeswitch = document.getElementById('modeswitch');

let selectedFile = null;

// EPUB target version (Optimize) + the experimental 3.4 image-format choice.
const verpills = document.getElementById('verpills');
const imgfmtpills = document.getElementById('imgfmtpills');
const ver34note = document.getElementById('ver34note');
let ver = '3.3';
let imgfmt = 'avif';

// Per-mode configuration: which file type, which i18n keys, which endpoint.
const MODE_CFG = {
  optimize: { accept: '.epub',  dropKey: 'drop_title',       hKey: 'opt_h',         ctaKey: 'cta',         workKey: 'cta_working',         readyKey: 'res_ready',         endpoint: '/convert' },
  archive:  { accept: '.epub',  dropKey: 'drop_title',       hKey: 'opt_h_archive', ctaKey: 'cta_archive', workKey: 'cta_working_archive', readyKey: 'res_ready_archive', endpoint: '/archive' },
  restore:  { accept: '.eparc', dropKey: 'drop_title_eparc', hKey: 'opt_h_restore', ctaKey: 'cta_restore', workKey: 'cta_working_restore', readyKey: 'res_ready_restore', endpoint: '/restore' },
};
let mode = 'optimize';

['dragenter','dragover'].forEach(e => drop.addEventListener(e, ev => { ev.preventDefault(); drop.classList.add('drag'); }));
['dragleave','drop'].forEach(e => drop.addEventListener(e, ev => { ev.preventDefault(); drop.classList.remove('drag'); }));
drop.addEventListener('drop', ev => { const f = ev.dataTransfer.files[0]; if (f) pickFile(f); });
file.addEventListener('change', () => { if (file.files[0]) pickFile(file.files[0]); });

function pickFile(f){ selectedFile = f; chipname.textContent = f.name; chip.classList.add('show'); }
function fmtBytes(n){ if (n >= 1048576) return (n/1048576).toFixed(2)+' MB'; if (n >= 1024) return (n/1024).toFixed(1)+' KB'; return n+' B'; }

// ---- mode switching ----------------------------------------------------------
function applyMode(m){
  mode = m;
  const cfg = MODE_CFG[m];

  modeswitch.querySelectorAll('.mode').forEach(b => {
    const on = b.dataset.mode === m;
    b.classList.toggle('on', on);
    b.setAttribute('aria-selected', on ? 'true' : 'false');
  });

  file.setAttribute('accept', cfg.accept);
  // Update the i18n keys on the dynamic labels so they also survive a language
  // switch, then render them immediately for the current language.
  dropTitle.setAttribute('data-i18n-html', cfg.dropKey); dropTitle.innerHTML = T(cfg.dropKey);
  optsH.setAttribute('data-i18n', cfg.hKey);             optsH.textContent = T(cfg.hKey);
  ctaLabel.setAttribute('data-i18n', cfg.ctaKey);        ctaLabel.textContent = T(cfg.ctaKey);

  updateOptionVisibility();

  // A file picked for one type rarely fits another (.epub vs .eparc) — start fresh.
  selectedFile = null; file.value = ''; chip.classList.remove('show');
  result.classList.remove('show');
}

// Show only the option rows that belong to the current mode. Rows tagged data-mz
// are the optimizer controls reused by Restore — only shown there when
// "Modernize" is on. (Kept separate from applyMode so toggling Modernize doesn't
// reset the already-dropped file.)
function updateOptionVisibility(){
  document.querySelectorAll('.opts [data-modes]').forEach(el => {
    const modes = el.getAttribute('data-modes').split(/\s+/);
    let show = modes.includes(mode);
    if (show && mode === 'restore' && el.hasAttribute('data-mz')) show = modernize.checked;
    // Version-specific controls (Optimize only): a data-ver gate.
    if (show && mode === 'optimize' && el.dataset.ver) show = el.dataset.ver === ver;
    el.classList.toggle('hide', !show);
  });
  // The 3.4 explainer shows only when Optimize + 3.4 is selected.
  if (ver34note) ver34note.classList.toggle('hide', !(mode === 'optimize' && ver === '3.4'));
}

modeswitch.querySelectorAll('.mode').forEach(b => {
  b.addEventListener('click', () => applyMode(b.dataset.mode));
});
modernize.addEventListener('change', () => { if (mode === 'restore') updateOptionVisibility(); });

// Target version pills (Optimize).
if (verpills) verpills.querySelectorAll('.pill').forEach(p => {
  p.addEventListener('click', () => {
    ver = p.dataset.ver;
    verpills.querySelectorAll('.pill').forEach(x => x.classList.toggle('on', x === p));
    updateOptionVisibility();
  });
});

// 3.4 image-format pills (Keep original / AVIF / JPEG XL).
function setImgfmt(fmt, locked) {
  imgfmt = fmt;
  if (!imgfmtpills) return;
  imgfmtpills.querySelectorAll('.pill').forEach(x => {
    x.classList.toggle('on', x.dataset.fmt === fmt);
    x.classList.toggle('locked', locked);
  });
}
if (imgfmtpills) imgfmtpills.querySelectorAll('.pill').forEach(p => {
  p.addEventListener('click', () => {
    if (p.classList.contains('locked')) return;
    setImgfmt(p.dataset.fmt, false);
  });
});

// ---- submit ------------------------------------------------------------------
go.addEventListener('click', async () => {
  if (!selectedFile){ drop.classList.add('drag'); setTimeout(()=>drop.classList.remove('drag'),350); return; }
  const cfg = MODE_CFG[mode];
  // Swap only the label text (not go.innerHTML) so the #ctaLabel element — and
  // its later per-mode updates — survive; keep the icon intact.
  const prev = ctaLabel.textContent;
  go.disabled = true; go.style.opacity = .7; ctaLabel.textContent = T(cfg.workKey);
  try {
    const fd = new FormData();
    fd.append('file', selectedFile);
    if (mode === 'optimize'){
      fd.append('quality', q.value);
      fd.append('ascii', ascii.checked ? 'true' : 'false');
      fd.append('kepub', kepub.checked ? 'true' : 'false');
      fd.append('target', ver);
      if (ver === '3.4'){
        // 3.4: the image-format pills choose Keep original / AVIF / JPEG XL.
        if (imgfmt === 'keep') fd.append('keep_images', 'true');
        else fd.append('image_format', imgfmt); // avif | jxl
      } else {
        fd.append('keep_images', keepImages.checked ? 'true' : 'false');
      }
    } else if (mode === 'archive'){
      fd.append('ascii', ascii.checked ? 'true' : 'false');
    } else if (mode === 'restore'){
      fd.append('modernize', modernize.checked ? 'true' : 'false');
      if (modernize.checked){
        fd.append('quality', q.value);
        fd.append('kepub', kepub.checked ? 'true' : 'false');
        fd.append('keep_images', keepImages.checked ? 'true' : 'false');
      }
    }
    // archive sends only the file.
    const res = await fetch(cfg.endpoint, { method:'POST', body: fd });
    if (!res.ok) {
      let msg = T('err_failed') + ' (HTTP ' + res.status + ').';
      try { const e = await res.json(); if (e && e.error) msg = e.error; } catch (_) {}
      throw new Error(msg);
    }
    const data = await res.json();
    renderResult(mode, data);
  } catch (err) {
    alert(err.message || T('err_generic'));
  } finally {
    go.disabled = false; go.style.opacity = 1; ctaLabel.textContent = prev;
  }
});

// ---- result rendering --------------------------------------------------------
const labBefore = document.getElementById('labBefore');
const labAfter  = document.getElementById('labAfter');
const labSaved  = document.getElementById('labSaved');

// Show only the result sections that belong to this mode.
function applyResultVisibility(m){
  document.querySelectorAll('[data-result-modes]').forEach(el => {
    el.classList.toggle('hide', !el.getAttribute('data-result-modes').split(/\s+/).includes(m));
  });
}

// Set a stat label's i18n key + text so it also survives a language switch.
function setLabel(el, key){ el.setAttribute('data-i18n', key); el.textContent = T(key); }

function fillStats(beforeBytes, afterBytes, savedPct){
  document.getElementById('sBefore').textContent = fmtBytes(beforeBytes);
  document.getElementById('sAfter').textContent  = fmtBytes(afterBytes);
  document.getElementById('sSaved').textContent  = Math.round(savedPct) + '%';
  const frac = beforeBytes > 0 ? Math.max(4, Math.min(100, afterBytes / beforeBytes * 100)) : 100;
  document.getElementById('sBar').style.width = frac + '%';
}

function renderResult(m, data){
  outname.textContent = data.output_name;
  resReady.setAttribute('data-i18n', MODE_CFG[m].readyKey);
  resReady.textContent = T(MODE_CFG[m].readyKey);

  const dl = document.getElementById('dl');
  dl.href = '/download/' + encodeURIComponent(data.download_token);
  dl.download = data.output_name;

  applyResultVisibility(m);
  resultSub.hidden = (m === 'optimize');

  if (m === 'optimize'){
    setLabel(labBefore, 'stat_before'); setLabel(labAfter, 'stat_after'); setLabel(labSaved, 'stat_saved');
    fillStats(data.original_size, data.final_size, data.saved_pct);
    renderImageReport(data);
  } else if (m === 'archive'){
    setLabel(labBefore, 'stat_original'); setLabel(labAfter, 'stat_archive'); setLabel(labSaved, 'stat_saved');
    fillStats(data.original_size, data.archive_size, data.saved_pct);
    resultSub.innerHTML = fill('sub_archive', {
      c: '<b>' + data.compressed_entries + '</b>',
      s: '<b>' + data.stored_entries + '</b>',
    });
  } else { // restore
    const sizeStr = '<b>' + fmtBytes(data.output_size) + '</b>';
    const key = data.modernized ? 'sub_restore_modernized' : 'sub_restore_exact';
    resultSub.innerHTML = fill(key, { n: '<b>' + data.entries + '</b>', size: sizeStr });
  }

  result.classList.remove('show'); void result.offsetWidth; result.classList.add('show');
  result.scrollIntoView({ behavior:'smooth', block:'center' });
}

// The convert-only image audit table + downloadable text report.
function renderImageReport(data){
  document.getElementById('imgCount').textContent = data.images.length;
  const tb = document.getElementById('itbody');
  tb.replaceChildren();
  const rows = data.images.slice().sort((a,b) => b.saved_pct - a.saved_pct);
  if (rows.length === 0){
    const tr = document.createElement('tr');
    const td = document.createElement('td');
    td.colSpan = 4; td.style.color = 'rgba(244,238,225,0.6)';
    td.textContent = T('tbl_noimages');
    tr.appendChild(td); tb.appendChild(tr);
  } else {
    for (const im of rows){
      const tr = document.createElement('tr');
      const c = (t, cls) => { const td = document.createElement('td'); td.textContent = t; if (cls) td.className = cls; return td; };
      tr.appendChild(c(im.name, 'name'));        // textContent => safe from HTML injection
      tr.appendChild(c(fmtBytes(im.before)));
      tr.appendChild(c(fmtBytes(im.after)));
      tr.appendChild(c(Math.round(im.saved_pct) + '%', 'pct'));
      tb.appendChild(tr);
    }
  }

  // The text report is small, so it rides along in the JSON.
  const txt = document.getElementById('txtdl');
  txt.href = URL.createObjectURL(new Blob([data.report_text], { type:'text/plain' }));
  txt.download = data.output_name.replace(/\.epub$/i, '') + '_report.txt';
}

// Kobo (.kepub) forces keep-original images (Kobo can't render WebP), so reflect
// that in the UI: tick and lock the "Keep original images" toggle while kepub is on.
let keepImagesPrev = keepImages.checked;
let imgfmtPrev = imgfmt;
kepub.addEventListener('change', () => {
  if (kepub.checked) {
    keepImagesPrev = keepImages.checked;
    keepImages.checked = true;
    keepImages.disabled = true;
    // Kobo can't render WebP/AVIF/JXL → lock the 3.4 selector to Keep original.
    imgfmtPrev = imgfmt;
    setImgfmt('keep', true);
  } else {
    keepImages.disabled = false;
    keepImages.checked = keepImagesPrev;
    setImgfmt(imgfmtPrev, false);
  }
});

const rtoggle = document.getElementById('rtoggle');
const report = document.getElementById('report');
const rtxt = document.getElementById('rtoggle-txt');
rtoggle.addEventListener('click', () => {
  const open = report.classList.toggle('open');
  rtoggle.classList.toggle('open', open);
  rtxt.textContent = open ? T('report_hide') : T('report_view');
});

// Keep the report toggle label correct when the language changes mid-view.
document.addEventListener('i18n:change', () => {
  rtxt.textContent = report.classList.contains('open') ? T('report_hide') : T('report_view');
});

// Initialize the default mode (also sets the per-mode option visibility).
applyMode('optimize');
