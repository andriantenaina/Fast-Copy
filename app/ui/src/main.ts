import { mount } from 'svelte';
import './styles.css';
import App from './App.svelte';

// No browser context menu / text selection outside inputs: this is an app, not a page.
window.addEventListener('contextmenu', (e) => {
  const el = e.target as HTMLElement | null;
  if (!el?.closest('input, textarea, .selectable')) e.preventDefault();
});

export default mount(App, { target: document.getElementById('app')! });
