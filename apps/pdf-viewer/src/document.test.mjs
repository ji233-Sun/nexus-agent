import test from 'node:test';
import assert from 'node:assert/strict';
import * as library from 'pdf-lib';
import { getDocument } from 'pdfjs-dist/legacy/build/pdf.mjs';
import { DocumentEdits, captureBounds, editedPdf } from './document.mjs';

test('page edits preserve annotation ownership and undo returns to the saved document', () => {
  const edits = new DocumentEdits([0, 90, 0]);
  edits.change(pages => pages[1].marks.push({ text: 'review', points: [[10, 20]] }));
  edits.markSaved();
  edits.rotate(1);
  edits.move(1, -1);
  assert.equal(edits.pages[0].rotation, 180);
  assert.equal(edits.pages[0].index, 1);
  assert.equal(edits.pages[0].marks[0].text, 'review');
  edits.remove(1);
  assert.equal(edits.pages.length, 2);
  assert.equal(edits.dirty, true);
  edits.undo(); edits.undo(); edits.undo();
  assert.equal(edits.dirty, false);
  assert.equal(edits.pages[1].marks.length, 1);
  edits.redo();
  assert.equal(edits.dirty, true);
  edits.change(pages => pages[1].marks[0].text = 'changed');
  assert.equal(edits.redo(), false);
  const single = new DocumentEdits([0]);
  assert.equal(single.remove(0), false);
  assert.equal(single.move(0, 1), 0);
});

test('capture bounds include edge pixels and clamp reversed or out-of-page selections', () => {
  assert.deepEqual(captureBounds([101.7, 52.1, 10.2, -5], 100, 200), { x: 10, y: 0, width: 90, height: 53 });
  assert.throws(() => captureBounds([120, 0, 130, 20], 100, 100));
});

test('export keeps selectable text, mixed rotations and crop boxes in a 100 page document', async () => {
  const source = await library.PDFDocument.create();
  for (let i = 0; i < 100; i++) {
    const page = source.addPage([600, 800]);
    page.setCropBox(20, 30, 540, 700);
    page.drawText(`Nexus page ${i + 1}`, { x: 60, y: 500 });
    page.setRotation(library.degrees(i % 2 ? 90 : 0));
  }
  const edits = new DocumentEdits(source.getPages().map(page => page.getRotation().angle));
  edits.rotate(0); edits.move(0, 1); edits.remove(2);
  const exported = await editedPdf(source, edits.pages, library);
  const saved = await library.PDFDocument.load(exported);
  assert.equal(saved.getPageCount(), 99);
  assert.deepEqual(saved.getPage(0).getCropBox(), { x: 20, y: 30, width: 540, height: 700 });
  const loading = getDocument({ data: exported, useSystemFonts: true, isEvalSupported: false });
  const reopened = await loading.promise;
  assert.equal(reopened.numPages, 99);
  const first = await reopened.getPage(1);
  assert.equal(first.rotate, 90);
  for (const [rotation, expected] of [
    [0, {x:80, y:300, width:120, height:160}],
    [90, {x:940, y:80, width:160, height:120}],
    [180, {x:880, y:940, width:120, height:160}],
    [270, {x:300, y:880, width:160, height:120}],
  ]) {
    const viewport = first.getViewport({scale:2, rotation});
    assert.deepEqual(captureBounds([60,500,120,580],viewport.width,viewport.height,viewport),expected);
  }
  assert.equal((await first.getTextContent()).items.map(item => item.str).join(''), 'Nexus page 2');
  assert.equal((await (await reopened.getPage(2)).getTextContent()).items.map(item => item.str).join(''), 'Nexus page 1');
  await loading.destroy();
});
