// Front-end logic for epublift-web. Served at /app.js as a same-origin script so
// the Content-Security-Policy can use `script-src 'self'` (no inline scripts).
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
const go = document.getElementById('go');
let selectedFile = null;

['dragenter','dragover'].forEach(e => drop.addEventListener(e, ev => { ev.preventDefault(); drop.classList.add('drag'); }));
['dragleave','drop'].forEach(e => drop.addEventListener(e, ev => { ev.preventDefault(); drop.classList.remove('drag'); }));
drop.addEventListener('drop', ev => { const f = ev.dataTransfer.files[0]; if (f) pickFile(f); });
file.addEventListener('change', () => { if (file.files[0]) pickFile(file.files[0]); });

function pickFile(f){ selectedFile = f; chipname.textContent = f.name; chip.classList.add('show'); }
function fmtBytes(n){ if (n >= 1048576) return (n/1048576).toFixed(2)+' MB'; if (n >= 1024) return (n/1024).toFixed(1)+' KB'; return n+' B'; }

go.addEventListener('click', async () => {
  if (!selectedFile){ drop.classList.add('drag'); setTimeout(()=>drop.classList.remove('drag'),350); return; }
  const label = go.innerHTML;
  go.disabled = true; go.style.opacity = .7; go.innerHTML = 'Lifting…';
  try {
    const fd = new FormData();
    fd.append('file', selectedFile);
    fd.append('quality', q.value);
    fd.append('ascii', ascii.checked ? 'true' : 'false');
    fd.append('kepub', kepub.checked ? 'true' : 'false');
    const res = await fetch('/convert', { method:'POST', body: fd });
    if (!res.ok) {
      let msg = 'Conversion failed (HTTP ' + res.status + ').';
      try { const e = await res.json(); if (e && e.error) msg = e.error; } catch (_) {}
      throw new Error(msg);
    }
    const data = await res.json();
    renderResult(data);
  } catch (err) {
    alert(err.message || 'Something went wrong.');
  } finally {
    go.disabled = false; go.style.opacity = 1; go.innerHTML = label;
  }
});

function renderResult(data){
  outname.textContent = data.output_name;
  document.getElementById('sBefore').textContent = fmtBytes(data.original_size);
  document.getElementById('sAfter').textContent  = fmtBytes(data.final_size);
  const pct = Math.round(data.saved_pct);
  document.getElementById('sSaved').textContent = pct + '%';
  const frac = data.original_size > 0 ? Math.max(4, Math.min(100, data.final_size / data.original_size * 100)) : 100;
  document.getElementById('sBar').style.width = frac + '%';

  document.getElementById('imgCount').textContent = data.images.length;
  const tb = document.getElementById('itbody');
  tb.replaceChildren();
  const rows = data.images.slice().sort((a,b) => b.saved_pct - a.saved_pct);
  if (rows.length === 0){
    const tr = document.createElement('tr');
    const td = document.createElement('td');
    td.colSpan = 4; td.style.color = 'rgba(244,238,225,0.6)';
    td.textContent = 'No raster images needed conversion.';
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

  // The converted EPUB streams straight from the server by one-time token.
  const dl = document.getElementById('dl');
  dl.href = '/download/' + encodeURIComponent(data.download_token);
  dl.download = data.output_name;

  // The text report is small, so it rides along in the JSON.
  const txt = document.getElementById('txtdl');
  txt.href = URL.createObjectURL(new Blob([data.report_text], { type:'text/plain' }));
  txt.download = data.output_name.replace(/\.epub$/i, '') + '_report.txt';

  result.classList.remove('show'); void result.offsetWidth; result.classList.add('show');
  result.scrollIntoView({ behavior:'smooth', block:'center' });
}

const rtoggle = document.getElementById('rtoggle');
const report = document.getElementById('report');
const rtxt = document.getElementById('rtoggle-txt');
rtoggle.addEventListener('click', () => {
  const open = report.classList.toggle('open');
  rtoggle.classList.toggle('open', open);
  rtxt.textContent = open ? 'Hide full report' : 'View full report';
});
