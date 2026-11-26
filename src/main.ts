import { mount } from 'svelte';
import './app.css';
import App from './App.svelte';

const target = document.getElementById('app');

if (!target) {
  throw new Error('找不到应用挂载节点 #app');
}

const app = mount(App, { target });

export default app;
