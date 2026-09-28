/* Workbench report image viewer: click any image for a full-size preview with
   arrow-key browsing, 100% / fit and download. No dependencies, no network calls.
   Adapted from Mr. Mak Workspace (MIT), workspace/_shared/report.js. */
(() => {
  if (window.__wbReportViewer) return;
  window.__wbReportViewer = true;
  const ready = () => {
    const images = () => [...document.querySelectorAll('img')].filter((i) => !i.closest('.wb-lightbox'));
    let dialog, stage, display, caption, counter, download, original, note, zoom, restoreFocus;
    let current = 0;
    const button = (label, action) => {
      const b = document.createElement('button');
      b.type = 'button';
      b.textContent = label;
      b.addEventListener('click', action);
      return b;
    };
    const build = () => {
      dialog = document.createElement('dialog');
      dialog.className = 'wb-lightbox';
      dialog.setAttribute('aria-label', 'Image preview');
      const bar = document.createElement('div');
      bar.className = 'wb-lightbox-toolbar';
      caption = document.createElement('span');
      caption.className = 'wb-lightbox-caption';
      counter = document.createElement('span');
      original = document.createElement('a');
      original.target = '_blank';
      original.rel = 'noreferrer';
      original.textContent = 'Open original';
      download = document.createElement('a');
      download.textContent = 'Download';
      zoom = button('100%', () => {
        stage.classList.toggle('actual');
        zoom.textContent = stage.classList.contains('actual') ? 'Fit' : '100%';
      });
      bar.append(caption, counter, button('Previous', () => change(-1)), button('Next', () => change(1)), zoom, original, download, button('Close', () => dialog.close()));
      stage = document.createElement('div');
      stage.className = 'wb-lightbox-stage';
      display = document.createElement('img');
      stage.append(display);
      display.addEventListener('click', () => zoom.click());
      note = document.createElement('p');
      note.className = 'wb-lightbox-note';
      note.setAttribute('role', 'status');
      display.addEventListener('error', () => {
        note.textContent = 'Image unavailable. Try Open original.';
      });
      dialog.append(bar, stage, note);
      document.body.append(dialog);
      dialog.addEventListener('close', () => restoreFocus?.focus({ preventScroll: true }));
      dialog.addEventListener('keydown', (e) => {
        if (e.key === 'ArrowRight') {
          e.preventDefault();
          change(1);
        }
        if (e.key === 'ArrowLeft') {
          e.preventDefault();
          change(-1);
        }
      });
      dialog.addEventListener('click', (e) => {
        if (e.target === stage) dialog.close();
      });
    };
    const show = (image) => {
      if (!dialog) build();
      const all = images();
      current = all.indexOf(image);
      const url = image.currentSrc || image.src;
      let name = 'image';
      try {
        name = decodeURIComponent(new URL(url, location.href).pathname.split('/').pop()) || name;
      } catch {
        /* data: URLs keep the fallback name */
      }
      caption.textContent = image.alt || name;
      counter.textContent = `${current + 1} / ${all.length}`;
      display.src = url;
      display.alt = image.alt || name;
      original.href = url;
      download.href = url;
      download.download = name;
      stage.classList.remove('actual');
      zoom.textContent = '100%';
      note.textContent = 'Arrow keys to browse · Esc to close';
      if (!dialog.open) {
        restoreFocus = image;
        dialog.showModal();
      }
    };
    const change = (d) => {
      const all = images();
      if (all.length) show(all[(current + d + all.length) % all.length]);
    };
    const prepare = () => {
      for (const img of document.querySelectorAll('img:not([data-wb-zoom])')) {
        if (img.closest('.wb-lightbox') || img.closest('a[href]')) continue;
        img.dataset.wbZoom = 'true';
        if (!img.hasAttribute('tabindex')) img.tabIndex = 0;
      }
    };
    prepare();
    new MutationObserver(prepare).observe(document.body, { childList: true, subtree: true });
    document.addEventListener(
      'click',
      (e) => {
        const img = e.target.closest?.('img[data-wb-zoom]');
        if (!img || e.ctrlKey || e.metaKey || e.shiftKey || e.altKey) return;
        e.preventDefault();
        show(img);
      },
      true,
    );
    document.addEventListener(
      'keydown',
      (e) => {
        if ((e.key === 'Enter' || e.key === ' ') && e.target.matches?.('img[data-wb-zoom]')) {
          e.preventDefault();
          show(e.target);
        }
      },
      true,
    );
  };
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', ready, { once: true });
  else ready();
})();
