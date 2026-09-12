// Catch failures before the viewer has installed its own document error handler.
import('./viewer.mjs').catch(error => {
  const status = document.getElementById('status');
  status.textContent = `PDF 预览加载失败 / Failed to load PDF: ${error.message}`;
  status.classList.add('error');
  for (const element of document.querySelectorAll('button, input, select')) element.disabled = true;
});
