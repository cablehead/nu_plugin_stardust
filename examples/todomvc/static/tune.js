// tune.js - a panel for trying other Stellar tokens on the transitions
// between notes, loaded only while tuning is on (see the gate in
// templates/note.html). Each pick is saved, with the CSS it makes, in
// localStorage under "tune"; the gate applies that CSS on the next page,
// before it is first shown, so the next navigation runs with it.

(() => {
  const durations = ['instant', 'fast', 'base', 'slow', '-2', '-1', '0', '1', '2', '3', '4'];
  const eases = ['standard', 'emphasized', 'entrance', 'pulse', 'linear', 'bounce'];
  const fields = [
    {key: 'contents', label: 'contents', options: durations, token: (v) => `--anim-duration-${v}`, fallback: 'fast'},
    {key: 'page', label: 'page fade', options: durations, token: (v) => `--anim-duration-${v}`, fallback: 'fast'},
    {key: 'ease', label: 'easing', options: eases, token: (v) => `--anim-ease-${v}`, fallback: 'standard'},
  ];

  let picks = {};
  try {
    picks = JSON.parse(localStorage.getItem('tune'))?.picks ?? {};
  } catch {}

  const css = () => {
    const pick = (f) => `var(${f.token(picks[f.key] ?? f.fallback)})`;
    const [contents, page, ease] = fields.map(pick);
    return `::view-transition-group(*) { animation-duration: ${contents}; animation-timing-function: ${ease}; }\n` +
      `::view-transition-group(root) { animation-duration: ${page}; }`;
  };

  const save = () => {
    const text = css();
    try {
      localStorage.setItem('tune', JSON.stringify({picks, css: text}));
    } catch {}
    document.getElementById('tune-style').textContent = text;
  };

  // A duration token is a calc() of the viewport; a probe element resolves
  // it to seconds for the option labels.
  const seconds = (token) => {
    const probe = document.createElement('div');
    probe.style.transitionDuration = `var(${token})`;
    document.body.append(probe);
    const s = parseFloat(getComputedStyle(probe).transitionDuration);
    probe.remove();
    return `${s.toFixed(2)}s`;
  };

  const panelCss = `
    #tune {
      position: fixed;
      right: var(--size-0);
      bottom: var(--size-0);
      display: grid;
      grid-template-columns: auto auto;
      gap: var(--size--2) var(--size--1);
      align-items: center;
      padding: var(--size--1);
      border: var(--border-width-1) solid var(--neutral-4);
      border-radius: var(--border-radius-2);
      background: var(--neutral-2);
      color: var(--neutral-2-on);
      font-size: var(--font-size--1);
      box-shadow: var(--shadow-2);
    }
    #tune button {
      grid-column: 1 / -1;
      font: inherit;
      color: var(--neutral-2-dim);
      background: none;
      border: none;
      cursor: pointer;
      justify-self: end;
    }
    #tune select {
      font: inherit;
    }`;

  const build = () => {
    const style = document.createElement('style');
    style.textContent = panelCss;
    document.head.append(style);

    const panel = document.createElement('form');
    panel.id = 'tune';
    for (const f of fields) {
      const label = document.createElement('label');
      label.textContent = f.label;
      label.htmlFor = `tune-${f.key}`;
      const select = document.createElement('select');
      select.id = `tune-${f.key}`;
      for (const v of f.options) {
        const option = new Option(f.options === durations ? `${v} · ${seconds(f.token(v))}` : v, v);
        option.selected = v === (picks[f.key] ?? f.fallback);
        select.append(option);
      }
      select.addEventListener('change', () => {
        picks[f.key] = select.value;
        save();
      });
      panel.append(label, select);
    }
    const off = document.createElement('button');
    off.type = 'button';
    off.textContent = 'stop tuning';
    off.addEventListener('click', () => {
      try {
        localStorage.removeItem('tune');
      } catch {}
      document.getElementById('tune-style').textContent = '';
      panel.remove();
    });
    panel.append(off);
    document.body.append(panel);
    save();
  };

  if (document.body) build();
  else document.addEventListener('DOMContentLoaded', build);
})();
