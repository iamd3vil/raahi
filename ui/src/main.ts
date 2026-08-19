import { mount } from 'svelte';
import '@knadh/oat/oat.min.css';
import '@knadh/oat/oat.min.js';
import './app.css';
import App from './App.svelte';

const app = mount(App, { target: document.getElementById('app')! });

export default app;
