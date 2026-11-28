import { mount } from 'svelte';
import './app.css';
import AvatarWindow from './routes/avatar/AvatarWindow.svelte';

const target = document.getElementById('app');

if (!target) {
  throw new Error('找不到应用挂载节点 #app');
}

const app = mount(AvatarWindow, { target });

export default app;
