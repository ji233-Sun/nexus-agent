import * as pdfjs from './vendor/pdf.mjs';
import * as pdfLibrary from './vendor/pdf-lib.mjs';
import { DocumentEdits, rectangle, captureBounds, editedPdf } from './document.mjs';
import { paintMarks, hitMark } from './marks.mjs';

const { EventBus, PDFViewer, PDFLinkService, PDFFindController } = await import('./vendor/pdf_viewer.mjs');
pdfjs.GlobalWorkerOptions.workerSrc = new URL('./vendor/pdf.worker.mjs', import.meta.url).href;
const $ = id => document.getElementById(id);
const metadata = await fetch('meta');
if (!metadata.ok) throw new Error('无法读取文档信息 / Document metadata is unavailable');
const meta = await metadata.json();
const english = meta.language === 'en';
const t = (zh, en) => english ? en : zh;
document.documentElement.lang = meta.language;
document.title = `${meta.name} · Nexus PDF`;
$('filename').textContent = meta.name;
const labels = {
  '另存为': 'Save as', '保存': 'Save', '关闭': 'Close', '页面与目录': 'Pages / outline', '适应宽度': 'Fit width', '整页': 'Fit page',
  '查找下一个': 'Find next', '阅读与选择文字': 'Read / select text', '选择与移动标注': 'Select / move marks', '高亮': 'Highlight',
  '下划线': 'Underline', '画笔': 'Pen', '矩形': 'Rectangle', '箭头': 'Arrow', '添加文本': 'Add text', '文字批注': 'Note',
  '编辑标注': 'Edit mark', '删除标注': 'Delete mark', '撤销': 'Undo', '重做': 'Redo', '旋转页面': 'Rotate', '前移': 'Move earlier',
  '后移': 'Move later', '删除页面': 'Delete page', '框选加入聊天': 'Capture region', '整页加入聊天': 'Capture page', '取消': 'Cancel', '确认': 'Confirm',
  '上一页': 'Previous page', '下一页': 'Next page', '页码': 'Page number', '缩小': 'Zoom out', '放大': 'Zoom in', '缩放': 'Zoom', '搜索文档': 'Search document', '工具': 'Tool', '标注颜色': 'Mark color',
};
if (english) {
  for (const element of document.querySelectorAll('[data-label]')) element.textContent = labels[element.dataset.label];
  for (const element of document.querySelectorAll('[aria-label]')) element.setAttribute('aria-label', labels[element.getAttribute('aria-label')] ?? element.getAttribute('aria-label'));
  $('find').placeholder = 'Search document';
}
const eventBus = new EventBus();
const linkService = new PDFLinkService({ eventBus });
const findController = new PDFFindController({ eventBus, linkService });
const viewer = new PDFViewer({
  container: $('viewerContainer'), viewer: $('viewer'), eventBus, linkService, findController,
  annotationEditorMode: pdfjs.AnnotationEditorType.DISABLE,
  annotationMode: pdfjs.AnnotationMode.ENABLE,
  imageResourcesPath: new URL('vendor/images/', location.href).href,
  maxCanvasPixels: 12_000_000, enableDetailCanvas: false,
});
linkService.setViewer(viewer);
let original, current, source, edits, selected, busy = false, tool = 'read', generation = 0;
let thumbsObserver;

function status(message, error = false) {
  $('status').textContent = message;
  $('status').classList.toggle('error', error);
}
async function request(path, body, type) {
  const response = await fetch(path, { method: 'POST', headers: { 'Content-Type': type }, body });
  if (!response.ok) throw new Error(await response.text());
  return response.json();
}
async function dialog(title, description = '', text) {
  if ($('dialog').open) return null;
  $('dialog').returnValue = 'cancel';
  $('dialogTitle').textContent = title;
  $('dialogDescription').textContent = description;
  $('dialogText').hidden = text === undefined || text === 'password';
  $('dialogPassword').hidden = text !== 'password';
  $('dialogText').value = text === 'password' ? '' : text ?? '';
  $('dialogPassword').value = '';
  $('dialogText').setAttribute('aria-label', title);
  $('dialogPassword').setAttribute('aria-label', title);
  const done = new Promise(resolve => $('dialog').addEventListener('close', () => {
    resolve($('dialog').returnValue === 'ok' ? (text === 'password' ? $('dialogPassword').value : $('dialogText').value || true) : null);
  }, { once: true }));
  $('dialog').showModal();
  return done;
}
async function perform(action) {
  if (busy) return;
  busy = true; updateControls();
  try { await action(); } catch (error) { status(error.message, true); }
  finally { busy = false; updateControls(); }
}
function pageIndex() { return viewer.currentPageNumber - 1; }
function pageMarks() { return edits?.pages[pageIndex()]?.marks ?? []; }
function updateControls() {
  window.ipc?.postMessage(JSON.stringify({ dirty: edits?.dirty ?? false, busy }));
  $('modified').textContent = edits?.dirty ? t('未保存', 'Unsaved') : '';
  $('undo').disabled = busy || !edits?.past.length;
  $('redo').disabled = busy || !edits?.future.length;
  for (const id of ['save', 'saveAs', 'rotate', 'deletePage', 'moveBefore', 'moveAfter', 'tool', 'color']) $(id).disabled = busy || !source;
  for (const id of ['capturePage', 'captureRegion', 'pageNumber', 'previous', 'next']) $(id).disabled = busy || !current;
  $('deleteMark').disabled = busy || !selected;
  $('editMark').disabled = busy || !pageMarks().some(mark => mark.id === selected && mark.text);
  if (edits) {
    $('deletePage').disabled ||= edits.pages.length === 1;
    $('moveBefore').disabled ||= pageIndex() === 0;
    $('moveAfter').disabled ||= pageIndex() === edits.pages.length - 1;
  }
}
function setTool(value) {
  tool = value;
  if (value !== 'capture') $('tool').value = value;
  for (const canvas of document.querySelectorAll('.captureLayer')) canvas.className = `captureLayer ${tool === 'read' ? 'read' : tool === 'select' ? 'select' : 'draw'}`;
}
function repaint() {
  for (let i = 0; i < viewer.pagesCount; i++) {
    const view = viewer.getPageView(i), canvas = view.div.querySelector('.captureLayer');
    if (canvas) drawLayer(canvas, view, edits.pages[i].marks);
  }
  updateControls();
}
function drawLayer(canvas, view, marks) {
  const ratio = Math.min(devicePixelRatio || 1, 2, Math.sqrt(12_000_000 / (view.viewport.width * view.viewport.height)));
  const width = Math.ceil(view.viewport.width * ratio), height = Math.ceil(view.viewport.height * ratio);
  if (canvas.width !== width || canvas.height !== height) { canvas.width = width; canvas.height = height; }
  canvas.style.width = `${view.viewport.width}px`; canvas.style.height = `${view.viewport.height}px`;
  const context = canvas.getContext('2d');
  context.clearRect(0, 0, canvas.width, canvas.height);
  context.save(); context.scale(ratio, ratio);
  paintMarks(context, marks, view.viewport, selected);
  context.restore();
}
function mountLayer(view, index) {
  view.div.querySelector('.captureLayer')?.remove();
  const canvas = document.createElement('canvas');
  canvas.className = `captureLayer ${tool === 'read' ? 'read' : tool === 'select' ? 'select' : 'draw'}`;
  view.div.append(canvas);
  drawLayer(canvas, view, edits.pages[index].marks);
  const position = event => {
    const bounds = canvas.getBoundingClientRect();
    return view.viewport.convertToPdfPoint(
      Math.max(0, Math.min(view.viewport.width, (event.clientX - bounds.left) * view.viewport.width / bounds.width)),
      Math.max(0, Math.min(view.viewport.height, (event.clientY - bounds.top) * view.viewport.height / bounds.height)),
    );
  };
  let drag;
  canvas.addEventListener('pointerdown', event => {
    if (event.button !== 0 || busy || !edits || (tool !== 'capture' && !source)) return;
    event.preventDefault();
    viewer.currentPageNumber = index + 1;
    const point = position(event);
    const mark = tool === 'select' ? hitMark(edits.pages[index].marks, point, 5 / view.viewport.scale) : null;
    selected = mark?.id;
    drag = { start: point, points: [point], mark: mark && structuredClone(mark), tool };
    canvas.setPointerCapture(event.pointerId);
    repaint();
  });
  canvas.addEventListener('pointermove', event => {
    if (!drag) return;
    const point = position(event);
    if (drag.tool === 'pencil') drag.points.push(point); else drag.points = [drag.start, point];
    const marks = structuredClone(edits.pages[index].marks);
    if (drag.mark) {
      const moved = marks.find(mark => mark.id === drag.mark.id);
      moved.points = drag.mark.points.map(p => [p[0] + point[0] - drag.start[0], p[1] + point[1] - drag.start[1]]);
    } else if (!['read', 'select', 'text', 'note'].includes(drag.tool)) {
      marks.push({ kind: drag.tool === 'capture' ? 'rectangle' : drag.tool, points: drag.points, color: drag.tool === 'capture' ? '#2563eb' : $('color').value });
    }
    drawLayer(canvas, view, marks);
  });
  canvas.addEventListener('pointercancel', () => { drag = null; repaint(); });
  canvas.addEventListener('pointerup', event => {
    if (!drag) return;
    const drawing = drag, point = position(event); drag = null;
    const points = drawing.tool === 'pencil' ? [...drawing.points, point] : [drawing.start, point];
    void perform(async () => {
      if (drawing.tool === 'capture') {
        if (Math.hypot(point[0] - drawing.start[0], point[1] - drawing.start[1]) > 3) await capture(index, rectangle(drawing.start, point));
        setTool('read');
      } else if (drawing.mark) {
        edits.change(pages => {
          pages[index].marks.find(mark => mark.id === drawing.mark.id).points = drawing.mark.points.map(p => [p[0] + point[0] - drawing.start[0], p[1] + point[1] - drawing.start[1]]);
        });
      } else if (['text', 'note'].includes(drawing.tool)) {
        const text = await dialog(t('输入文字', 'Enter text'), '', '');
        if (typeof text === 'string' && text.trim()) addMark(index, { kind: drawing.tool, points: [drawing.start], text: text.trim() });
      } else if (!['read', 'select'].includes(drawing.tool) && points.length >= 2) {
        addMark(index, { kind: drawing.tool, points });
      }
      repaint();
    });
  });
}
function addMark(index, data) {
  const mark = { ...data, color: $('color').value, id: crypto.randomUUID() };
  edits.change(pages => pages[index].marks.push(mark));
  selected = mark.id;
}
eventBus.on('pagerendered', ({ pageNumber, error }) => {
  if (error) { status(error.message, true); return; }
  if (edits && pageNumber <= edits.pages.length) mountLayer(viewer.getPageView(pageNumber - 1), pageNumber - 1);
});
eventBus.on('pagechanging', ({ pageNumber }) => {
  selected = null;
  $('pageNumber').value = pageNumber;
  for (const button of document.querySelectorAll('.thumbnail')) button.setAttribute('aria-current', String(Number(button.dataset.page) === pageNumber));
  updateControls();
});
eventBus.on('updatefindmatchescount', ({ matchesCount }) => { $('matches').textContent = `${matchesCount.current} / ${matchesCount.total}`; });
eventBus.on('updatefindcontrolstate', ({ state }) => { if (state === 1) $('matches').textContent = t('无匹配', 'No matches'); });
eventBus.on('scalechanging', ({ scale, presetValue }) => {
  $('customZoom')?.remove();
  const value = presetValue || String(scale);
  if (![...$('zoom').options].some(option => option.value === value)) {
    const option = new Option(`${Math.round(scale * 100)}%`, value);
    option.id = 'customZoom'; $('zoom').add(option);
  }
  $('zoom').value = value;
});

async function loadPdf(data) {
  let cancelled = false;
  const task = pdfjs.getDocument({
    data: data.slice(), cMapUrl: new URL('vendor/cmaps/', location.href).href, cMapPacked: true,
    standardFontDataUrl: new URL('vendor/standard_fonts/', location.href).href,
    wasmUrl: new URL('vendor/wasm/', location.href).href,
    iccUrl: new URL('vendor/iccs/', location.href).href,
    isEvalSupported: false, enableXfa: false,
  });
  task.onPassword = async (setPassword, reason) => {
    const password = await dialog(t('PDF 密码', 'PDF password'), reason === 2 ? t('密码不正确，请重试。', 'Incorrect password. Try again.') : t('此文档需要密码。', 'This document requires a password.'), 'password');
    if (typeof password === 'string') setPassword(password); else { cancelled = true; void task.destroy(); }
  };
  return task.promise.catch(error => {
    if (cancelled) throw new Error(t('已取消打开加密文档。', 'Opening the encrypted document was cancelled.'));
    throw error;
  });
}
async function display(pdf, page = 1) {
  const old = current;
  current = pdf;
  generation++;
  const ready = new Promise(resolve => {
    const listener = () => { eventBus.off('pagesinit', listener); resolve(); };
    eventBus.on('pagesinit', listener);
  });
  viewer.setDocument(pdf); linkService.setDocument(pdf);
  await ready;
  viewer.currentScaleValue = $('zoom').value || 'page-width';
  viewer.currentPageNumber = Math.min(Math.max(page, 1), pdf.numPages);
  $('pageCount').textContent = `/ ${pdf.numPages}`; $('pageNumber').max = pdf.numPages;
  buildThumbnails();
  if (old && old !== original && old !== pdf) await old.loadingTask.destroy();
  updateControls();
}
function buildThumbnails() {
  thumbsObserver?.disconnect();
  $('thumbnails').replaceChildren();
  const epoch = generation, pdf = current;
  thumbsObserver = new IntersectionObserver(entries => {
    for (const entry of entries) if (entry.isIntersecting) {
      thumbsObserver.unobserve(entry.target);
      void (async () => {
        const index = Number(entry.target.dataset.page) - 1;
        const page = await pdf.getPage(index + 1);
        const viewport = page.getViewport({ scale: 110 / page.getViewport({ scale: 1 }).width });
        const canvas = document.createElement('canvas'); canvas.width = Math.ceil(viewport.width); canvas.height = Math.ceil(viewport.height);
        await page.render({ canvasContext: canvas.getContext('2d'), viewport }).promise;
        if (epoch === generation) entry.target.prepend(canvas);
      })().catch(() => {});
    }
  }, { root: $('sidebar'), rootMargin: '200px' });
  edits.pages.forEach((page, index) => {
    const button = document.createElement('button'); button.className = 'thumbnail'; button.dataset.page = index + 1;
    button.textContent = `${index + 1}`; button.setAttribute('aria-label', t(`第 ${index + 1} 页`, `Page ${index + 1}`));
    button.addEventListener('click', () => { viewer.currentPageNumber = index + 1; });
    $('thumbnails').append(button); thumbsObserver.observe(button);
  });
}
async function buildOutline() {
  const outline = await original.getOutline();
  const walk = (items, depth) => { for (const item of items ?? []) {
    const button = document.createElement('button'); button.textContent = item.title; button.style.paddingLeft = `${depth * 10 + 4}px`;
    button.addEventListener('click', () => void perform(async () => {
      const dest = typeof item.dest === 'string' ? await original.getDestination(item.dest) : item.dest;
      if (!dest) return;
      const index = typeof dest[0] === 'number' ? dest[0] : await original.getPageIndex(dest[0]);
      const currentIndex = edits.pages.findIndex(page => page.index === index);
      if (currentIndex >= 0) viewer.currentPageNumber = currentIndex + 1;
      else status(t('此目录指向的页面已删除。', 'The page for this outline entry was deleted.'));
    }));
    $('outline').append(button); walk(item.items, depth + 1);
  } };
  walk(outline, 0);
}
async function rebuild(page = viewer.currentPageNumber) {
  selected = null;
  const data = await editedPdf(source, edits.pages, pdfLibrary);
  await display(await loadPdf(data), page);
}
async function capture(index, selection) {
  status(t('正在生成截图…', 'Capturing…'));
  const page = await current.getPage(index + 1), base = page.getViewport({ scale: 1 });
  const scale = Math.min(Math.max(2, viewer.currentScale * 4 / 3), Math.sqrt(12_000_000 / (base.width * base.height)));
  const viewport = page.getViewport({ scale });
  const full = document.createElement('canvas'); full.width = Math.ceil(viewport.width); full.height = Math.ceil(viewport.height);
  const context = full.getContext('2d');
  await page.render({ canvasContext: context, viewport, annotationMode: pdfjs.AnnotationMode.ENABLE }).promise;
  paintMarks(context, edits.pages[index].marks, viewport);
  const bounds = captureBounds(selection ?? [0, 0, full.width, full.height], full.width, full.height, selection ? viewport : undefined);
  const result = document.createElement('canvas'); result.width = bounds.width; result.height = bounds.height;
  result.getContext('2d').drawImage(full, bounds.x, bounds.y, bounds.width, bounds.height, 0, 0, bounds.width, bounds.height);
  const blob = await new Promise(resolve => result.toBlob(resolve, 'image/png'));
  full.width = full.height = 0;
  if (!blob || blob.size > 5 * 1024 * 1024) throw new Error(t('截图超过 5 MiB，请框选较小的区域。', 'Capture exceeds 5 MiB. Select a smaller region.'));
  await request(`capture/${edits.pages[index].index + 1}`, blob, 'image/png');
  status(t('截图已加入当前聊天草稿，可返回聊天预览并发送。', 'Capture added to the current chat draft. Return to chat to preview and send.'));
}
async function save(copy) {
  status(t('正在准备 PDF…', 'Preparing PDF…'));
  const overlays = [];
  for (const item of edits.pages) {
    if (!item.marks.length) { overlays.push(null); continue; }
    const page = await original.getPage(item.index + 1);
    const base = page.getViewport({ scale: 1, rotation: 0 });
    const viewport = page.getViewport({ scale: Math.min(2, Math.sqrt(12_000_000 / (base.width * base.height))), rotation: 0 });
    const canvas = document.createElement('canvas'); canvas.width = Math.ceil(viewport.width); canvas.height = Math.ceil(viewport.height);
    paintMarks(canvas.getContext('2d'), item.marks, viewport);
    const blob = await new Promise(resolve => canvas.toBlob(resolve, 'image/png'));
    overlays.push({ bytes: new Uint8Array(await blob.arrayBuffer()), box: viewport.viewBox });
    canvas.width = canvas.height = 0;
  }
  const bytes = await editedPdf(source, edits.pages, pdfLibrary, overlays);
  const result = await request(`save?copy=${copy}`, bytes, 'application/pdf');
  if (result.saved) { edits.markSaved(); status(t(`已保存：${result.name}`, `Saved: ${result.name}`)); }
  else status(t('已取消保存，修改仍保留。', 'Save cancelled. Your edits are retained.'));
}
window.requestClose = async () => {
  if (busy || $('dialog').open) return;
  if (edits?.dirty && await dialog(t('关闭未保存的文档？', 'Close an unsaved document?'), t('关闭会丢弃尚未保存的修改。可以取消后保存或另存为。', 'Closing discards unsaved edits. Cancel to save a copy first.')) === null) return;
  await request('close', '', 'text/plain');
};
$('close').onclick = () => void window.requestClose();
$('save').onclick = () => void perform(() => save(false));
$('saveAs').onclick = () => void perform(() => save(true));
$('previous').onclick = () => { if (viewer.currentPageNumber > 1) viewer.currentPageNumber--; };
$('next').onclick = () => { if (viewer.currentPageNumber < viewer.pagesCount) viewer.currentPageNumber++; };
$('pageNumber').onchange = () => { const page = Math.trunc(Number($('pageNumber').value)); if (page >= 1 && page <= viewer.pagesCount) viewer.currentPageNumber = page; else $('pageNumber').value = viewer.currentPageNumber; };
$('zoom').onchange = () => { viewer.currentScaleValue = $('zoom').value; };
$('zoomIn').onclick = () => { viewer.currentScale = Math.min(4, viewer.currentScale * 1.2); };
$('zoomOut').onclick = () => { viewer.currentScale = Math.max(0.25, viewer.currentScale / 1.2); };
$('sidebarToggle').onclick = () => document.body.classList.toggle('sidebarHidden');
const find = (again = false) => eventBus.dispatch('find', { source: window, type: again ? 'again' : '', query: $('find').value, phraseSearch: true, caseSensitive: false, entireWord: false, highlightAll: true, findPrevious: false });
$('find').oninput = () => find(); $('findNext').onclick = () => find(true);
$('find').onkeydown = event => { if (event.key === 'Enter') { event.preventDefault(); find(true); } };
$('tool').onchange = () => setTool($('tool').value);
$('color').onchange = () => { if (selected) { edits.change(pages => { const mark = pages[pageIndex()].marks.find(mark => mark.id === selected); if (mark) mark.color = $('color').value; }); repaint(); } };
$('deleteMark').onclick = () => { edits.change(pages => { pages[pageIndex()].marks = pageMarks().filter(mark => mark.id !== selected); }); selected = null; repaint(); };
$('editMark').onclick = () => void perform(async () => {
  const mark = pageMarks().find(mark => mark.id === selected); if (!mark?.text) return;
  const text = await dialog(t('编辑标注', 'Edit mark'), '', mark.text);
  if (typeof text === 'string' && text.trim()) edits.change(() => { mark.text = text.trim(); });
  repaint();
});
$('undo').onclick = () => void perform(async () => { if (edits.undo()) await rebuild(); });
$('redo').onclick = () => void perform(async () => { if (edits.redo()) await rebuild(); });
$('rotate').onclick = () => void perform(async () => { edits.rotate(pageIndex()); await rebuild(); });
$('moveBefore').onclick = () => void perform(async () => { const page = edits.move(pageIndex(), -1); await rebuild(page + 1); });
$('moveAfter').onclick = () => void perform(async () => { const page = edits.move(pageIndex(), 1); await rebuild(page + 1); });
$('deletePage').onclick = () => void perform(async () => {
  if (await dialog(t('删除当前页？', 'Delete this page?'), t('保存前可以撤销此操作。', 'You can undo this edit.')) !== null && edits.remove(pageIndex())) await rebuild();
});
$('capturePage').onclick = () => void perform(() => capture(pageIndex()));
$('captureRegion').onclick = () => { setTool('capture'); status(t('在页面上拖动框选，松开后加入聊天。', 'Drag a region on the page; release to add it to chat.')); };
window.addEventListener('keydown', event => {
  if ($('dialog').open || ['INPUT', 'TEXTAREA', 'SELECT'].includes(event.target.tagName)) return;
  if ((event.metaKey || event.ctrlKey) && ['z', 's', 'f'].includes(event.key.toLowerCase())) {
    event.preventDefault();
    if (event.key.toLowerCase() === 'z') $(event.shiftKey ? 'redo' : 'undo').click();
    if (event.key.toLowerCase() === 's') $(event.shiftKey ? 'saveAs' : 'save').click();
    if (event.key.toLowerCase() === 'f') $('find').focus();
  }
  if (event.key === 'Escape') { setTool('read'); selected = null; repaint(); }
  if (['Delete', 'Backspace'].includes(event.key) && selected) { event.preventDefault(); $('deleteMark').click(); }
});
window.addEventListener('unhandledrejection', event => { status(event.reason?.message ?? String(event.reason), true); });

await perform(async () => {
  status(t('正在打开 PDF…', 'Opening PDF…'));
  const response = await fetch('document');
  if (!response.ok) throw new Error(t('无法读取 PDF。', 'Cannot read PDF.'));
  const bytes = new Uint8Array(await response.arrayBuffer());
  original = await loadPdf(bytes);
  const rotations = [];
  for (let i = 1; i <= original.numPages; i++) rotations.push((await original.getPage(i)).rotate);
  edits = new DocumentEdits(rotations);
  try { source = await pdfLibrary.PDFDocument.load(bytes, { updateMetadata: false }); } catch { source = null; }
  await display(original);
  await buildOutline();
  status(source ? t('可选择文字、搜索、标注，或截图加入聊天。', 'Select text, search, annotate, or capture a region for chat.') : t('此文档可预览和截图；加密或特殊格式暂不支持编辑保存。', 'Preview and capture are available. Editing encrypted or special-format PDFs is not supported.'));
});
