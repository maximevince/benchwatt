import { mount } from 'svelte';
import App from './App.svelte';
import './style.css';
mount(App, { target: document.getElementById('app')! });
// Hide the webview's browser menu (Back/Reload/Inspect Element). Text fields keep it for
// cut/copy/paste; in dev builds Shift+right-click still opens it for the inspector.
window.addEventListener('contextmenu', event => {
  const target = event.target as HTMLElement;
  if (target.closest('input, textarea, [contenteditable="true"]')) return;
  if (import.meta.env.DEV && event.shiftKey) return;
  event.preventDefault();
});
