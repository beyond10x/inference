function linkListbox(): void {
  for (const input of document.querySelectorAll<HTMLInputElement>('input.navbar__search-input[role="combobox"][aria-owns]')) {
    const listbox = input.getAttribute('aria-owns');
    if (listbox && input.getAttribute('aria-controls') !== listbox) input.setAttribute('aria-controls', listbox);
  }
}

if (typeof window !== 'undefined') {
  new MutationObserver(linkListbox).observe(document.documentElement, {
    subtree: true,
    attributes: true,
    attributeFilter: ['role', 'aria-owns'],
  });
}
