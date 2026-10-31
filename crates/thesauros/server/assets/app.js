'use strict';
document.addEventListener('htmx:responseError', (event) => {
  const notice = document.getElementById('notice');
  if (!notice) return;
  let message = 'Request failed. Retry.';
  try { message = JSON.parse(event.detail.xhr.responseText).error || message; } catch {}
  if (event.detail.xhr.status === 401) {
    location.assign('/');
    return;
  }
  notice.textContent = message;
  notice.classList.add('text-amber-300');
});
document.addEventListener('htmx:sendError', () => {
  const notice = document.getElementById('notice');
  if (notice) {
    notice.textContent = 'Connection lost. Retry the request.';
    notice.classList.add('text-amber-300');
  }
});
document.addEventListener('htmx:xhr:progress', (event) => {
  const progress = event.detail.elt.querySelector('progress');
  if (progress && event.detail.total) {
    progress.value = event.detail.loaded / event.detail.total * 100;
  }
});
document.addEventListener('htmx:afterRequest', (event) => {
  if (event.detail.successful && event.detail.elt.id === 'upload-form') event.detail.elt.reset();
});

// Preserve the user's job-history disclosure choice across HTMX refreshes.
let jobsOpen;
document.addEventListener('click', (event) => {
  if (!(event.target instanceof Element)) return;
  const summary = event.target.closest('#jobs > summary');
  if (summary) jobsOpen = !summary.parentElement.open;
});
document.addEventListener('htmx:afterSwap', (event) => {
  if (event.detail.target.id === 'jobs' && typeof jobsOpen === 'boolean') {
    const jobs = document.getElementById('jobs');
    if (jobs) jobs.open = jobsOpen;
  }
});

document.addEventListener('htmx:beforeSwap', (event) => {
  const { target, xhr } = event.detail;
  if (target.id === 'notice' && xhr.status >= 200 && xhr.status < 300) {
    target.classList.remove('text-amber-300');
  }
});

// Searchable selectors submit only catalog choices; the search text is local UI state.
(() => {
  const pickers = new Map();
  let active;
  let snapshot = new Map();
  const sidebar = document.getElementById('library-sidebar');
  const sidebarToggle = document.querySelector('.sidebar-toggle');
  const content = document.querySelector('.workspace-content');
  const header = document.querySelector('.app-header');
  const desktop = matchMedia('(min-width: 768px)');
  const focusable = (root) => [...root.querySelectorAll('button, a[href], input, select, summary, [tabindex="0"]')]
    .filter((element) => !element.disabled && !element.hidden && element.getClientRects().length);

  function closePicker(focus = false) {
    if (!active) return;
    const entry = active;
    entry.menu.hidden = true;
    entry.trigger.setAttribute('aria-expanded', 'false');
    entry.search.setAttribute('aria-expanded', 'false');
    entry.search.removeAttribute('aria-activedescendant');
    active = undefined;
    if (focus && entry.trigger.isConnected) entry.trigger.focus({ preventScroll: true });
  }
  function positionPicker() {
    if (!active) return;
    const { menu, trigger } = active;
    const anchor = trigger.getBoundingClientRect();
    const viewport = window.visualViewport;
    const height = viewport?.height || innerHeight;
    const width = Math.min(Math.max(anchor.width, 240), innerWidth - 16);
    const below = height - anchor.bottom - 14;
    const above = anchor.top - 14;
    const down = below >= 160 || below >= above;
    menu.style.width = `${width}px`;
    menu.style.maxHeight = `${Math.max(100, Math.min(360, down ? below : above))}px`;
    menu.style.left = `${Math.max(8, Math.min(anchor.left, innerWidth - width - 8))}px`;
    menu.style.top = `${Math.max(8, Math.min(down ? anchor.bottom + 6 : anchor.top - menu.offsetHeight - 6, height - menu.offsetHeight - 8))}px`;
  }
  function activate(entry, option) {
    entry.options.forEach((item) => item.classList.toggle('is-active', item === option));
    entry.activeOption = option;
    if (option) {
      entry.search.setAttribute('aria-activedescendant', option.id);
      option.scrollIntoView({ block: 'nearest' });
    } else entry.search.removeAttribute('aria-activedescendant');
  }
  function filter(entry) {
    const query = entry.search.value.trim().toLocaleLowerCase();
    entry.options.forEach((option) => {
      option.hidden = query !== '' && (option.dataset.value === '' || !option.textContent.toLocaleLowerCase().includes(query));
    });
    const visible = entry.options.filter((option) => !option.hidden);
    entry.empty.hidden = visible.length !== 0;
    activate(entry, visible.find((option) => option.dataset.value === entry.native.value) || visible[0]);
    positionPicker();
  }
  function openPicker(entry, query = '') {
    closePicker();
    (sidebar && !desktop.matches && document.body.classList.contains('sidebar-open') ? sidebar : document.body).append(entry.menu);
    active = entry;
    entry.menu.hidden = false;
    entry.search.value = query;
    entry.trigger.setAttribute('aria-expanded', 'true');
    entry.search.setAttribute('aria-expanded', 'true');
    positionPicker();
    filter(entry);
    entry.search.focus({ preventScroll: true });
  }
  function setSelection(entry, value, submit = false) {
    if (![...entry.native.options].some((option) => option.value === value)) {
      entry.native.add(new Option(value, value));
      const option = document.createElement('li');
      option.id = `${entry.root.dataset.picker}-option-retained`;
      option.role = 'option';
      option.className = 'picker-option';
      option.dataset.value = value;
      option.textContent = value;
      option.setAttribute('aria-label', value);
      entry.menu.querySelector('[role="listbox"]').append(option);
      entry.options.push(option);
    }
    entry.native.value = value;
    entry.value.textContent = entry.native.selectedOptions[0].textContent;
    entry.options.forEach((option) => option.setAttribute('aria-selected', String(option.dataset.value === value)));
    if (submit) entry.native.dispatchEvent(new Event('change', { bubbles: true }));
  }
  function initPickers() {
    for (const [id, entry] of pickers) {
      if (document.getElementById(id) !== entry.root) {
        if (active === entry) closePicker();
        entry.menu.remove();
        pickers.delete(id);
      }
    }
    for (const root of document.querySelectorAll('[data-picker]')) {
      if (pickers.has(root.id)) continue;
      const entry = {
        root, native: root.querySelector('[data-picker-native]'),
        trigger: root.querySelector('[data-picker-trigger]'), value: root.querySelector('[data-picker-value]'),
        menu: root.querySelector('[data-picker-menu]'),
      };
      entry.search = entry.menu.querySelector('[data-picker-search]');
      entry.empty = entry.menu.querySelector('[data-picker-empty]');
      entry.options = [...entry.menu.querySelectorAll('[role="option"]')];
      entry.trigger.id = `${root.dataset.picker}-trigger`;
      root.querySelector('label').htmlFor = entry.trigger.id;
      entry.native.hidden = true;
      entry.trigger.hidden = false;
      document.body.append(entry.menu);
      pickers.set(root.id, entry);
      const previous = snapshot.get(root.id);
      setSelection(entry, previous?.value ?? entry.native.value);
      if (previous?.open) openPicker(entry, previous.query);
    }
    snapshot.clear();
  }
  function setSidebar(open, restoreFocus = false) {
    if (!sidebar) return;
    closePicker();
    const modal = open && !desktop.matches;
    document.body.classList.toggle('sidebar-open', modal);
    sidebarToggle.setAttribute('aria-expanded', String(modal));
    sidebar.inert = !desktop.matches && !modal;
    sidebar.setAttribute('aria-hidden', String(sidebar.inert));
    if (modal) { sidebar.setAttribute('role', 'dialog'); sidebar.setAttribute('aria-modal', 'true'); }
    else { sidebar.removeAttribute('role'); sidebar.removeAttribute('aria-modal'); }
    content.inert = modal;
    header.inert = modal;
    if (modal) focusable(sidebar)[0]?.focus({ preventScroll: true });
    else if (restoreFocus) sidebarToggle.focus({ preventScroll: true });
  }
  document.addEventListener('click', (event) => {
    if (!(event.target instanceof Element)) return;
    const trigger = event.target.closest('[data-picker-trigger]');
    const menu = event.target.closest('[data-picker-menu]');
    if (trigger) {
      const entry = pickers.get(trigger.closest('[data-picker]').id);
      if (entry === active) closePicker(true); else openPicker(entry);
    } else if (menu && event.target.closest('[role="option"]')) {
      const entry = pickers.get(menu.dataset.pickerMenu);
      const option = event.target.closest('[role="option"]');
      if (!option.hidden) { setSelection(entry, option.dataset.value, true); closePicker(true); }
    } else if (active && !active.root.contains(event.target) && !active.menu.contains(event.target)) closePicker();
    if (event.target.closest('.sidebar-toggle')) setSidebar(!document.body.classList.contains('sidebar-open'));
    if (event.target.closest('[data-sidebar-close]')) setSidebar(false, true);
  });
  document.addEventListener('input', (event) => {
    if (active && event.target === active.search) filter(active);
  });
  document.addEventListener('change', (event) => {
    if (event.target instanceof Element && event.target.matches('[data-auto-submit]')) {
      document.getElementById('search-form').requestSubmit();
    }
  });
  document.addEventListener('keydown', (event) => {
    if (event.key === 'Escape') {
      if (active) { event.preventDefault(); closePicker(true); return; }
      if (document.body.classList.contains('sidebar-open')) { event.preventDefault(); setSidebar(false, true); }
      return;
    }
    const trigger = event.target instanceof Element && event.target.closest('[data-picker-trigger]');
    if (trigger && ['ArrowDown', 'ArrowUp'].includes(event.key)) {
      event.preventDefault(); openPicker(pickers.get(trigger.closest('[data-picker]').id)); return;
    }
    if (active && event.target === active.search) {
      const visible = active.options.filter((option) => !option.hidden);
      if (['ArrowDown', 'ArrowUp'].includes(event.key)) {
        event.preventDefault();
        const index = visible.indexOf(active.activeOption);
        activate(active, visible[(index + (event.key === 'ArrowDown' ? 1 : -1) + visible.length) % visible.length]);
      } else if (event.key === 'Enter') {
        event.preventDefault();
        if (active.activeOption) { const entry = active; setSelection(entry, entry.activeOption.dataset.value, true); closePicker(true); }
      } else if (event.key === 'Tab') closePicker(true);
    }
    if (event.key === 'Tab' && sidebar && document.body.classList.contains('sidebar-open')) {
      const elements = focusable(sidebar);
      const first = elements[0], last = elements[elements.length - 1];
      if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last?.focus(); }
      else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first?.focus(); }
    }
  });
  let galleryScroll = 0;
  document.addEventListener('htmx:beforeSwap', (event) => {
    if (event.detail.target.id === 'gallery') {
      galleryScroll = event.detail.requestConfig?.elt?.id === 'gallery' ? event.detail.target.scrollTop : 0;
    }
    if (event.detail.target.id !== 'filters') return;
    snapshot = new Map([...pickers].map(([id, entry]) => [id, { value: entry.native.value, query: entry.search.value, open: active === entry }]));
  });
  document.addEventListener('htmx:afterSwap', (event) => {
    if (event.detail.target.id === 'filters') initPickers();
    if (event.detail.target.id === 'gallery') document.getElementById('gallery').scrollTop = galleryScroll;
  });
  document.addEventListener('scroll', (event) => {
    if (active && !active.menu.contains(event.target)) positionPicker();
  }, true);
  window.addEventListener('resize', positionPicker);
  window.visualViewport?.addEventListener('resize', positionPicker);
  desktop.addEventListener('change', () => setSidebar(false));
  initPickers();
  setSidebar(false);
})();
