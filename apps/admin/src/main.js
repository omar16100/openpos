import { mount } from 'svelte';

// The screen's own look, loaded here rather than held inside a component: see
// the note at the top of the file.
import './screen.css';
import App from './App.svelte';

export default mount(App, { target: document.getElementById('app') });
