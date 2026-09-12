import { rectangle } from './document.mjs';

export function markBounds(mark) {
  const xs = mark.points.map(p => p[0]), ys = mark.points.map(p => p[1]);
  const box = [Math.min(...xs), Math.min(...ys), Math.max(...xs), Math.max(...ys)];
  if (mark.text) { box[2] += 240; box[1] -= mark.text.split('\n').length * 17 + 12; }
  return box;
}

export function hitMark(marks, point, tolerance) {
  return [...marks].reverse().find(mark => {
    const [x1, y1, x2, y2] = markBounds(mark);
    return point[0] >= x1 - tolerance && point[0] <= x2 + tolerance && point[1] >= y1 - tolerance && point[1] <= y2 + tolerance;
  });
}

// All marks use PDF coordinates. The same painter serves preview, capture and export.
export function paintMarks(context, marks, viewport, selected) {
  context.save();
  context.transform(...viewport.transform);
  context.lineCap = 'round';
  context.lineJoin = 'round';
  for (const mark of marks) {
    context.save();
    context.strokeStyle = context.fillStyle = mark.color;
    context.lineWidth = 2;
    const a = mark.points[0], b = mark.points.at(-1);
    const [x1, y1, x2, y2] = rectangle(a, b);
    if (mark.kind === 'highlight') {
      context.globalAlpha = 0.3;
      context.fillRect(x1, y1, x2 - x1, y2 - y1);
    } else if (mark.kind === 'rectangle') {
      context.strokeRect(x1, y1, x2 - x1, y2 - y1);
    } else if (mark.text) {
      context.translate(a[0], a[1]);
      context.scale(1, -1);
      context.font = '12px system-ui, sans-serif';
      context.textBaseline = 'top';
      const lines = mark.text.split('\n');
      if (mark.kind === 'note') {
        context.fillStyle = '#fff4b8';
        context.fillRect(-4, -4, 248, lines.length * 17 + 8);
      }
      context.fillStyle = mark.color;
      lines.forEach((line, i) => context.fillText(line, 0, i * 17, 240));
    } else {
      context.beginPath();
      context.moveTo(...a);
      for (const point of mark.points.slice(1)) context.lineTo(...point);
      if (mark.kind === 'arrow') {
        const angle = Math.atan2(b[1] - a[1], b[0] - a[0]);
        for (const offset of [-0.5, 0.5]) {
          context.moveTo(...b);
          context.lineTo(b[0] - 10 * Math.cos(angle + offset), b[1] - 10 * Math.sin(angle + offset));
        }
      }
      context.stroke();
    }
    context.restore();
    if (mark.id === selected) {
      context.save();
      context.strokeStyle = '#2563eb';
      context.lineWidth = 1 / viewport.scale;
      context.setLineDash([4 / viewport.scale, 3 / viewport.scale]);
      const [x, y, right, top] = markBounds(mark);
      context.strokeRect(x - 3, y - 3, right - x + 6, top - y + 6);
      context.restore();
    }
  }
  context.restore();
}
