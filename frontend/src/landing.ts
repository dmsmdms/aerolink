// Landing page: static HTML in English, translated in place (works without JS, just in English).
import './styles.css';
import { lang, t, toggleLang, type Key } from './lib/i18n';

document.documentElement.lang = lang;
for (const el of document.querySelectorAll<HTMLElement>('[data-i18n]')) {
  el.textContent = t(el.dataset.i18n as Key);
}
document.getElementById('lang')?.addEventListener('click', toggleLang);
