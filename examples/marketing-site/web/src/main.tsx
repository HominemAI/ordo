import { lazy, StrictMode, Suspense } from 'react';
import { createRoot } from 'react-dom/client';
import { App } from './App.js';
import './styles.css';
const Studio = lazy(() => import('./Studio.js').then((module) => ({ default: module.Studio })));

const root = document.getElementById('root');
if (!root) throw new Error('index.html has no #root');
createRoot(root).render(
  <StrictMode>
    {window.location.pathname === '/studio' ? (
      <Suspense
        fallback={
          <main>
            <p role="status">Opening studio…</p>
          </main>
        }
      >
        <Studio />
      </Suspense>
    ) : (
      <App />
    )}
  </StrictMode>,
);
