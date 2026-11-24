import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { App } from './App.js';
import './styles.css';
import { Studio } from './Studio.js';

const root = document.getElementById('root');
if (!root) throw new Error('index.html has no #root');
createRoot(root).render(
  <StrictMode>
    {window.location.pathname === '/studio' ? <Studio /> : <App />}
  </StrictMode>,
);
