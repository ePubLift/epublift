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
const kepubWebp = document.getElementById('kepub_webp');
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
  import:   { accept: '.pdf',   dropKey: 'drop_title_pdf',   hKey: 'opt_h_import',  ctaKey: 'cta_import',  workKey: 'cta_working_import',  readyKey: 'res_ready_import',  endpoint: '/import' },
  metadata: { accept: '.epub',  dropKey: 'drop_title' },
};
const importLang = document.getElementById('importLang');

// Metadata editor elements.
const ctaRow = document.getElementById('ctaRow');
const metaForm = document.getElementById('metaForm');
const mStatus = document.getElementById('m_status');
const mDone = document.getElementById('m_done');
const mFetch = document.getElementById('m_fetch');
const mSave = document.getElementById('m_save');
let mode = 'optimize';

['dragenter','dragover'].forEach(e => drop.addEventListener(e, ev => { ev.preventDefault(); drop.classList.add('drag'); }));
['dragleave','drop'].forEach(e => drop.addEventListener(e, ev => { ev.preventDefault(); drop.classList.remove('drag'); }));
drop.addEventListener('drop', ev => { const f = ev.dataTransfer.files[0]; if (f) pickFile(f); });
file.addEventListener('change', () => { if (file.files[0]) pickFile(file.files[0]); });

function pickFile(f){
  selectedFile = f; chipname.textContent = f.name; chip.classList.add('show');
  if (mode === 'metadata') loadMetadata(f);
}
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

  // The metadata editor has its own form + save button, so it hides the generic
  // options header and the global "go" CTA.
  const isMeta = m === 'metadata';
  ctaRow.classList.toggle('hide', isMeta);
  optsH.classList.toggle('hide', isMeta);
  if (!isMeta) {
    optsH.setAttribute('data-i18n', cfg.hKey);      optsH.textContent = T(cfg.hKey);
    ctaLabel.setAttribute('data-i18n', cfg.ctaKey); ctaLabel.textContent = T(cfg.ctaKey);
  }
  // Reset the metadata form (it re-reveals once a file is read).
  metaForm.classList.add('hide'); mDone.classList.add('hide');
  mStatus.textContent = ''; mStatus.classList.remove('warn');

  updateOptionVisibility();
  localizeBookLangs();

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
      fd.append('kepub_webp', kepubWebp.checked ? 'true' : 'false');
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
        fd.append('kepub_webp', kepubWebp.checked ? 'true' : 'false');
        fd.append('keep_images', keepImages.checked ? 'true' : 'false');
      }
    } else if (mode === 'import'){
      if (importLang) fd.append('language', importLang.value);
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
  } else if (m === 'import'){
    resultSub.innerHTML = fill('sub_import', {
      c: '<b>' + data.chapters + '</b>',
      p: '<b>' + data.paragraphs + '</b>',
      size: '<b>' + fmtBytes(data.final_size) + '</b>',
    });
  } else { // restore
    const sizeStr = '<b>' + fmtBytes(data.output_size) + '</b>';
    const key = data.modernized ? 'sub_restore_modernized' : 'sub_restore_exact';
    resultSub.innerHTML = fill(key, { n: '<b>' + data.entries + '</b>', size: sizeStr });
  }

  result.classList.remove('show'); void result.offsetWidth; result.classList.add('show');
  result.scrollIntoView({ behavior:'smooth', block:'center' });
}

// Human format name for the report header, from the current 3.3/3.4 selection.
function reportFmtName(){
  if (ver === '3.3') return 'WebP';
  if (imgfmt === 'avif') return 'AVIF';
  if (imgfmt === 'jxl') return 'JPEG XL';
  return 'WebP';
}
// Fill the two version/format-specific report headers (re-callable on lang change).
function fillReportLabels(){
  document.getElementById('rcolModernized').textContent = fill('rcol_modernized', { ver });
  document.getElementById('rcolImages').textContent = fill('rcol_images', { fmt: reportFmtName() });
}

// The convert-only image audit table + downloadable text report.
function renderImageReport(data){
  fillReportLabels();
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

// Keep the report toggle label + the version/format headers correct when the
// language changes mid-view (applyStatic would otherwise show raw placeholders).
document.addEventListener('i18n:change', () => {
  rtxt.textContent = report.classList.contains('open') ? T('report_hide') : T('report_view');
  if (result.classList.contains('show') && mode === 'optimize') fillReportLabels();
  updateDateHint(); // re-append the localized date format hint after applyStatic
  localizeBookLangs(); // re-localize the book-language names to the new UI language
});

// Footer build info: link the version to its GitHub release, and (when known)
// the commit to its GitHub commit. Cheap deploy-verification signal.
fetch('/version').then(r => r.json()).then(d => {
  const repo = 'https://github.com/ePubLift/epublift';
  if (d && d.version) {
    const v = document.getElementById('verlink');
    v.textContent = 'v' + d.version;
    v.href = repo + '/releases/tag/web-v' + d.version;
  }
  if (d && d.commit) {
    const c = document.getElementById('commitlink');
    c.textContent = '@' + d.commit;
    c.href = repo + '/commit/' + d.commit;
  }
}).catch(() => { /* version is non-essential; ignore */ });

// ---- metadata editor ---------------------------------------------------------
async function errMsg(res){
  let m = T('err_failed') + ' (HTTP ' + res.status + ').';
  try {
    const e = await res.json();
    if (e) {
      // Localize the common, expected failures by their code; fall back to the
      // server's English detail for anything unexpected.
      const errs = (fldL().err) || FLD.en.err;
      if (e.code && errs && errs[e.code]) m = errs[e.code];
      else if (e.error) m = e.error;
    }
  } catch (_) {}
  return m;
}
function setv(id, val){ document.getElementById(id).value = (val == null) ? '' : val; }
function seriesStr(s){ return s ? (s.position ? s.name + ':' + s.position : s.name) : ''; }
function curLang(){ return (window.i18n && window.i18n.lang) ? window.i18n.lang : 'en'; }

// Localize the "Book language" <select> labels into the CURRENT PAGE language
// (e.g. a Turkish UI shows "İngilizce, Almanca…"). The user is choosing the
// book's language here, not the page language, so the names follow the page
// language — unlike the top-right UI-language switcher, which uses endonyms.
// Uses the browser's built-in Intl.DisplayNames (no translation tables); older
// browsers keep the native-name fallbacks baked into the HTML.
function localizeBookLangs(){
  if (!importLang) return;
  let dn;
  try { dn = new Intl.DisplayNames([curLang()], { type: 'language' }); }
  catch (_) { return; }
  Array.from(importLang.options).forEach(o => {
    try {
      const name = dn.of(o.value);
      if (name) o.textContent = name.charAt(0).toLocaleUpperCase(curLang()) + name.slice(1);
    } catch (_) { /* keep the HTML fallback for this option */ }
  });
}

// --- author name: show as "First Last" (flip a single-comma "Last, First") ---
function authorDisplay(name){
  const m = /^([^,]+),\s*([^,]+)$/.exec((name || '').trim());
  return m ? (m[2].trim() + ' ' + m[1].trim()) : name;
}

// --- date: locale-aware display, ISO 8601 in the file -------------------------
// East-Asian locales use year-first; the rest day-first.
function ymdLang(l){ return l === 'ja' || l === 'ko' || l === 'zh'; }
const DATE_HINT = {
  en:'dd.mm.yyyy', es:'dd.mm.aaaa', tr:'gg.aa.yyyy', de:'tt.mm.jjjj', fr:'jj.mm.aaaa',
  pt:'dd.mm.aaaa', it:'gg.mm.aaaa', nl:'dd.mm.jjjj', pl:'dd.mm.rrrr', ru:'дд.мм.гггг',
  ja:'yyyy.mm.dd', ko:'yyyy.mm.dd', zh:'yyyy.mm.dd',
};
function pad2(n){ n = String(n); return n.length < 2 ? '0' + n : n; }
// ISO (yyyy-mm-dd) -> localized display; partial/non-ISO shown as-is.
function isoToDisplay(s){
  const m = /^(\d{4})-(\d{2})-(\d{2})/.exec(s || '');
  if (!m) return s || '';
  return ymdLang(curLang()) ? `${m[1]}.${m[2]}.${m[3]}` : `${m[3]}.${m[2]}.${m[1]}`;
}
// Localized display -> ISO yyyy-mm-dd (best effort; year-only/unknown kept as-is).
function displayToIso(s){
  s = (s || '').trim(); if (!s) return s;
  let m;
  if ((m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(s))) return s;
  if ((m = /^(\d{4})[.\/-](\d{1,2})[.\/-](\d{1,2})$/.exec(s))) return `${m[1]}-${pad2(m[2])}-${pad2(m[3])}`;
  if ((m = /^(\d{1,2})[.\/-](\d{1,2})[.\/-](\d{4})$/.exec(s))) return `${m[3]}-${pad2(m[2])}-${pad2(m[1])}`;
  return s;
}
// Append the localized format hint to the date field's label.
function updateDateHint(){
  const el = document.getElementById('m_date_label');
  if (el) el.textContent = T('meta_f_date') + ' (' + (DATE_HINT[curLang()] || DATE_HINT.en) + ')';
}

// Localized field names + skip reasons for the enrich status (server sends keys).
const FLD = {
  en: { f:{title:'title',subtitle:'subtitle',authors:'authors',publisher:'publisher',date:'date',isbn:'ISBN',subjects:'subjects',description:'description',series:'series'}, present:'already set', lang:'language mismatch', omitted:'description omitted', ed:"The matched edition is in '{e}', not '{b}' — those fields were skipped.", err:{rate_limited:"The provider's quota is exhausted (rate limited). Try again later, or set a Google Books API key.", not_found:"This ISBN wasn't found by the selected provider — try the other provider or check the number.", no_isbn:'Enter an ISBN first.', busy:'Too many requests — please wait a moment and try again.'} },
  tr: { f:{title:'başlık',subtitle:'alt başlık',authors:'yazarlar',publisher:'yayınevi',date:'tarih',isbn:'ISBN',subjects:'konular',description:'açıklama',series:'seri'}, present:'zaten var', lang:'dil uyuşmuyor', omitted:'açıklama atlandı', ed:"Eşleşen baskı '{b}' değil '{e}' dilinde — o alanlar atlandı.", err:{rate_limited:'Sağlayıcının kotası doldu (hız sınırı). Daha sonra tekrar dene ya da bir Google Books API anahtarı tanımla.', not_found:'Bu ISBN seçili sağlayıcıda bulunamadı — diğer sağlayıcıyı dene ya da numarayı kontrol et.', no_isbn:'Önce bir ISBN gir.', busy:'Çok fazla istek — lütfen biraz bekleyip tekrar dene.'} },
  es: { f:{title:'título',subtitle:'subtítulo',authors:'autores',publisher:'editorial',date:'fecha',isbn:'ISBN',subjects:'materias',description:'descripción',series:'serie'}, present:'ya está', lang:'idioma distinto', omitted:'descripción omitida', ed:"La edición encontrada está en '{e}', no en '{b}' — esos campos se omitieron.", err:{rate_limited:'Se agotó la cuota del proveedor (límite de peticiones). Inténtalo más tarde o configura una clave de API de Google Books.', not_found:'El proveedor seleccionado no encontró este ISBN — prueba el otro proveedor o revisa el número.', no_isbn:'Introduce primero un ISBN.', busy:'Demasiadas peticiones — espera un momento e inténtalo de nuevo.'} },
  de: { f:{title:'Titel',subtitle:'Untertitel',authors:'Autoren',publisher:'Verlag',date:'Datum',isbn:'ISBN',subjects:'Themen',description:'Beschreibung',series:'Reihe'}, present:'bereits vorhanden', lang:'andere Sprache', omitted:'Beschreibung übersprungen', ed:"Die gefundene Ausgabe ist in '{e}', nicht '{b}' — diese Felder wurden übersprungen.", err:{rate_limited:'Das Kontingent des Anbieters ist erschöpft (Rate-Limit). Später erneut versuchen oder einen Google-Books-API-Schlüssel setzen.', not_found:'Der gewählte Anbieter hat diese ISBN nicht gefunden — anderen Anbieter versuchen oder Nummer prüfen.', no_isbn:'Zuerst eine ISBN eingeben.', busy:'Zu viele Anfragen — bitte einen Moment warten und erneut versuchen.'} },
  fr: { f:{title:'titre',subtitle:'sous-titre',authors:'auteurs',publisher:'éditeur',date:'date',isbn:'ISBN',subjects:'sujets',description:'description',series:'série'}, present:'déjà défini', lang:'langue différente', omitted:'description ignorée', ed:"L'édition trouvée est en '{e}', pas en '{b}' — ces champs ont été ignorés.", err:{rate_limited:"Le quota du fournisseur est épuisé (limite de requêtes). Réessayez plus tard ou définissez une clé API Google Books.", not_found:"Le fournisseur sélectionné n'a pas trouvé cet ISBN — essayez l'autre fournisseur ou vérifiez le numéro.", no_isbn:"Saisissez d'abord un ISBN.", busy:'Trop de requêtes — patientez un instant puis réessayez.'} },
  pt: { f:{title:'título',subtitle:'subtítulo',authors:'autores',publisher:'editora',date:'data',isbn:'ISBN',subjects:'assuntos',description:'descrição',series:'série'}, present:'já definido', lang:'idioma diferente', omitted:'descrição omitida', ed:"A edição encontrada está em '{e}', não em '{b}' — esses campos foram ignorados.", err:{rate_limited:'A cota do provedor esgotou (limite de requisições). Tente mais tarde ou defina uma chave de API do Google Books.', not_found:'O provedor selecionado não encontrou este ISBN — tente o outro provedor ou verifique o número.', no_isbn:'Digite um ISBN primeiro.', busy:'Muitas requisições — aguarde um momento e tente novamente.'} },
  it: { f:{title:'titolo',subtitle:'sottotitolo',authors:'autori',publisher:'editore',date:'data',isbn:'ISBN',subjects:'soggetti',description:'descrizione',series:'collana'}, present:'già presente', lang:'lingua diversa', omitted:'descrizione saltata', ed:"L'edizione trovata è in '{e}', non in '{b}' — quei campi sono stati saltati.", err:{rate_limited:'Quota del fornitore esaurita (limite di richieste). Riprova più tardi o imposta una chiave API di Google Books.', not_found:"Il fornitore selezionato non ha trovato questo ISBN — prova l'altro fornitore o controlla il numero.", no_isbn:'Inserisci prima un ISBN.', busy:'Troppe richieste — attendi un momento e riprova.'} },
  nl: { f:{title:'titel',subtitle:'ondertitel',authors:'auteurs',publisher:'uitgever',date:'datum',isbn:'ISBN',subjects:'onderwerpen',description:'beschrijving',series:'reeks'}, present:'al ingevuld', lang:'andere taal', omitted:'beschrijving overgeslagen', ed:"De gevonden editie is in '{e}', niet '{b}' — die velden zijn overgeslagen.", err:{rate_limited:'Het quotum van de aanbieder is op (rate limit). Probeer later opnieuw of stel een Google Books API-sleutel in.', not_found:'De gekozen aanbieder vond deze ISBN niet — probeer de andere aanbieder of controleer het nummer.', no_isbn:'Voer eerst een ISBN in.', busy:'Te veel verzoeken — wacht even en probeer opnieuw.'} },
  pl: { f:{title:'tytuł',subtitle:'podtytuł',authors:'autorzy',publisher:'wydawca',date:'data',isbn:'ISBN',subjects:'tematy',description:'opis',series:'seria'}, present:'już jest', lang:'inny język', omitted:'opis pominięty', ed:"Znalezione wydanie jest w '{e}', nie '{b}' — te pola pominięto.", err:{rate_limited:'Wyczerpano limit dostawcy (rate limit). Spróbuj później lub ustaw klucz API Google Books.', not_found:'Wybrany dostawca nie znalazł tego ISBN — wypróbuj drugiego dostawcę lub sprawdź numer.', no_isbn:'Najpierw podaj ISBN.', busy:'Zbyt wiele żądań — odczekaj chwilę i spróbuj ponownie.'} },
  ru: { f:{title:'название',subtitle:'подзаголовок',authors:'авторы',publisher:'издательство',date:'дата',isbn:'ISBN',subjects:'темы',description:'описание',series:'серия'}, present:'уже задано', lang:'другой язык', omitted:'описание пропущено', ed:"Найденное издание на '{e}', а не '{b}' — эти поля пропущены.", err:{rate_limited:'Квота провайдера исчерпана (ограничение запросов). Повторите позже или задайте ключ API Google Books.', not_found:'Выбранный провайдер не нашёл этот ISBN — попробуйте другого провайдера или проверьте номер.', no_isbn:'Сначала введите ISBN.', busy:'Слишком много запросов — подождите немного и повторите.'} },
  ja: { f:{title:'タイトル',subtitle:'サブタイトル',authors:'著者',publisher:'出版社',date:'発行日',isbn:'ISBN',subjects:'件名',description:'説明',series:'シリーズ'}, present:'設定済み', lang:'言語が不一致', omitted:'説明はスキップ', ed:"見つかった版は '{b}' ではなく '{e}' です — これらの項目はスキップしました。", err:{rate_limited:'プロバイダーのクォータを使い切りました（レート制限）。後でもう一度試すか、Google Books の API キーを設定してください。', not_found:'選択したプロバイダーでこの ISBN が見つかりませんでした — 別のプロバイダーを試すか番号を確認してください。', no_isbn:'先に ISBN を入力してください。', busy:'リクエストが多すぎます — 少し待ってからもう一度お試しください。'} },
  ko: { f:{title:'제목',subtitle:'부제',authors:'저자',publisher:'출판사',date:'날짜',isbn:'ISBN',subjects:'주제',description:'설명',series:'시리즈'}, present:'이미 있음', lang:'언어 불일치', omitted:'설명 건너뜀', ed:"찾은 판이 '{b}'가 아닌 '{e}'입니다 — 해당 항목은 건너뛰었습니다.", err:{rate_limited:'공급자 할당량이 소진되었습니다(요청 제한). 나중에 다시 시도하거나 Google Books API 키를 설정하세요.', not_found:'선택한 공급자에서 이 ISBN을 찾지 못했습니다 — 다른 공급자를 시도하거나 번호를 확인하세요.', no_isbn:'먼저 ISBN을 입력하세요.', busy:'요청이 너무 많습니다 — 잠시 후 다시 시도하세요.'} },
  zh: { f:{title:'标题',subtitle:'副标题',authors:'作者',publisher:'出版社',date:'日期',isbn:'ISBN',subjects:'主题',description:'简介',series:'丛书'}, present:'已存在', lang:'语言不符', omitted:'已跳过简介', ed:"找到的版本为 '{e}'，而非 '{b}' —— 这些字段已跳过。", err:{rate_limited:'提供方配额已用尽（速率限制）。请稍后重试，或设置 Google Books API 密钥。', not_found:'所选提供方未找到该 ISBN——请尝试另一个提供方或核对号码。', no_isbn:'请先输入 ISBN。', busy:'请求过多——请稍候再试。'} },
};
function fldL(){ return FLD[curLang()] || FLD.en; }
function fldName(k){ return fldL().f[k] || k; }
function reasonText(r){ const L = fldL(); return r === 'present' ? L.present : r === 'lang' ? L.lang : L.omitted; }

function fillForm(md){
  setv('m_title', md.title); setv('m_subtitle', md.subtitle);
  setv('m_authors', (md.authors || []).map(authorDisplay).join('\n'));
  setv('m_language', md.language); setv('m_publisher', md.publisher);
  setv('m_date', isoToDisplay(md.date));
  setv('m_series', seriesStr(md.series));
  setv('m_subjects', (md.subjects || []).join('\n'));
  setv('m_description', md.description);
  if (md.isbn) setv('m_isbn', md.isbn);
  updateDateHint();
}

// Read the dropped book's current metadata and reveal the editable form.
async function loadMetadata(f){
  metaForm.classList.add('hide'); mDone.classList.add('hide');
  mStatus.classList.remove('warn'); mStatus.textContent = T('meta_reading');
  try {
    const fd = new FormData(); fd.append('file', f);
    const res = await fetch('/meta/read', { method:'POST', body: fd });
    if (!res.ok) throw new Error(await errMsg(res));
    fillForm(await res.json());
    metaForm.classList.remove('hide');
    mStatus.textContent = '';
  } catch (e) {
    mStatus.textContent = e.message || T('meta_read_err');
    mStatus.classList.add('warn');
  }
}

// Look up the ISBN and merge Open Library's language-aware suggestions in.
async function doEnrich(){
  if (!selectedFile) return;
  const isbn = document.getElementById('m_isbn').value.trim();
  if (!isbn){ mStatus.textContent = T('meta_isbn_required'); mStatus.classList.add('warn'); return; }
  const prev = mFetch.textContent;
  mFetch.disabled = true; mFetch.textContent = '…';
  mStatus.classList.remove('warn'); mStatus.textContent = T('meta_looking');
  try {
    const fd = new FormData();
    fd.append('file', selectedFile);
    fd.append('isbn', isbn);
    fd.append('provider', document.getElementById('m_provider').value);
    fd.append('lang', document.getElementById('m_language').value.trim());
    if (document.getElementById('m_allow_foreign').checked) fd.append('allow_foreign_meta', 'true');
    if (document.getElementById('m_include_desc').checked) fd.append('include_description', 'true');
    const res = await fetch('/meta/enrich', { method:'POST', body: fd });
    if (!res.ok) throw new Error(await errMsg(res));
    applyEnrich(await res.json());
  } catch (e) {
    mStatus.textContent = e.message || T('meta_lookup_err'); mStatus.classList.add('warn');
  } finally {
    mFetch.disabled = false; mFetch.textContent = prev;
  }
}

function applyEnrich(data){
  const f = data.fields || {};
  if (f.title != null) setv('m_title', f.title);
  if (f.subtitle != null) setv('m_subtitle', f.subtitle);
  if (f.authors != null) setv('m_authors', f.authors.map(authorDisplay).join('\n'));
  if (f.publisher != null) setv('m_publisher', f.publisher);
  if (f.date != null) setv('m_date', isoToDisplay(f.date));
  if (f.description != null) setv('m_description', f.description);
  if (f.subjects != null) setv('m_subjects', f.subjects.join('\n'));
  if (f.series != null) setv('m_series', seriesStr(f.series));
  if (f.isbn != null) setv('m_isbn', f.isbn);

  // Build a localized status from the structured applied/skipped/warnings.
  const lines = [];
  (data.warnings || []).forEach(w => {
    if (w.type === 'edition_lang') lines.push('⚠ ' + fldL().ed.replace('{e}', w.edition).replace('{b}', w.book));
  });
  const applied = (data.applied || []).map(a => a.value ? `${fldName(a.field)} = ${a.value}` : fldName(a.field));
  lines.push(applied.length ? T('meta_filled') + applied.join(', ') : T('meta_nothing'));
  const skipped = (data.skipped || []).map(s => `${fldName(s.field)} (${reasonText(s.reason)})`);
  if (skipped.length) lines.push(T('meta_skipped') + skipped.join(', '));
  mStatus.textContent = lines.join('\n');
  mStatus.classList.toggle('warn', (data.warnings || []).length > 0);
}

// Write the edited fields into the book and offer the result for download.
async function doSave(){
  if (!selectedFile) return;
  const fd = new FormData();
  fd.append('file', selectedFile);
  ['title','subtitle','language','publisher','series','isbn'].forEach(k => {
    const v = document.getElementById('m_' + k).value.trim();
    if (v) fd.append(k, v);
  });
  ['authors','subjects','description'].forEach(k => {
    const v = document.getElementById('m_' + k).value.trim();
    if (v) fd.append(k, v);
  });
  // Date: store ISO 8601 in the file regardless of the localized display format.
  const dateIso = displayToIso(document.getElementById('m_date').value);
  if (dateIso) fd.append('date', dateIso);
  const label = mSave.querySelector('span');
  const prev = label.textContent;
  mSave.disabled = true; mSave.style.opacity = .7; label.textContent = T('meta_saving');
  try {
    const res = await fetch('/meta/write', { method:'POST', body: fd });
    if (!res.ok) throw new Error(await errMsg(res));
    const data = await res.json();
    const dl = document.getElementById('m_dl');
    dl.href = '/download/' + encodeURIComponent(data.download_token);
    dl.download = data.output_name;
    mDone.classList.remove('hide');
    mDone.scrollIntoView({ behavior:'smooth', block:'center' });
  } catch (e) {
    alert(e.message || T('err_generic'));
  } finally {
    mSave.disabled = false; mSave.style.opacity = 1; label.textContent = prev;
  }
}

mFetch.addEventListener('click', doEnrich);
mSave.addEventListener('click', doSave);

// Initialize the default mode (also sets the per-mode option visibility).
applyMode('optimize');
