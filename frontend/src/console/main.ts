import { mount } from 'svelte';
import '../styles.css';
import { lang } from '../lib/i18n';
import App from './App.svelte';

document.documentElement.lang = lang;
mount(App, { target: document.getElementById('app')! });
