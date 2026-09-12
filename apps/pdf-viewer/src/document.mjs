export class DocumentEdits {
  constructor(rotations) {
    this.pages = rotations.map((rotation, index) => ({ index, rotation, marks: [] }));
    this.past = [];
    this.future = [];
    this.saved = JSON.stringify(this.pages);
  }
  get dirty() { return JSON.stringify(this.pages) !== this.saved; }
  markSaved() { this.saved = JSON.stringify(this.pages); }
  change(update) {
    const before = structuredClone(this.pages);
    update(this.pages);
    if (JSON.stringify(before) === JSON.stringify(this.pages)) return;
    this.past.push(before);
    if (this.past.length > 80) this.past.shift();
    this.future = [];
  }
  undo() {
    if (!this.past.length) return false;
    this.future.push(this.pages);
    this.pages = this.past.pop();
    return true;
  }
  redo() {
    if (!this.future.length) return false;
    this.past.push(this.pages);
    this.pages = this.future.pop();
    return true;
  }
  rotate(index) { this.change(pages => { pages[index].rotation = (pages[index].rotation + 90) % 360; }); }
  remove(index) {
    if (this.pages.length === 1) return false;
    this.change(pages => pages.splice(index, 1));
    return true;
  }
  move(index, delta) {
    const next = index + delta;
    if (next < 0 || next >= this.pages.length) return index;
    this.change(pages => { [pages[index], pages[next]] = [pages[next], pages[index]]; });
    return next;
  }
}

export function rectangle(a, b) {
  return [Math.min(a[0], b[0]), Math.min(a[1], b[1]), Math.max(a[0], b[0]), Math.max(a[1], b[1])];
}

export function captureBounds(rect, width, height, viewport) {
  const points = viewport ? [viewport.convertToViewportPoint(rect[0], rect[1]), viewport.convertToViewportPoint(rect[2], rect[3])] : [rect.slice(0, 2), rect.slice(2)];
  const [x1, y1, x2, y2] = rectangle(...points);
  const x = Math.max(0, Math.floor(x1)), y = Math.max(0, Math.floor(y1));
  const right = Math.min(width, Math.ceil(x2)), bottom = Math.min(height, Math.ceil(y2));
  if (right <= x || bottom <= y) throw new Error('Empty capture');
  return { x, y, width: right - x, height: bottom - y };
}

// Copy original pages, preserving their vector text, crop boxes and existing content.
export async function editedPdf(source, pages, library, overlays = []) {
  const output = await library.PDFDocument.create();
  const copies = await output.copyPages(source, pages.map(page => page.index));
  for (let i = 0; i < copies.length; i++) {
    const page = copies[i];
    page.setRotation(library.degrees(pages[i].rotation));
    output.addPage(page);
    if (overlays[i]) {
      const { bytes, box } = overlays[i];
      const image = await output.embedPng(bytes);
      page.drawImage(image, { x: box[0], y: box[1], width: box[2] - box[0], height: box[3] - box[1] });
    }
  }
  return output.save();
}
