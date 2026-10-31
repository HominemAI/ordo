import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { AdminPage } from './AdminPage.js';
import { LoginPage } from './LoginPage.js';
import { ShopPage } from './ShopPage.js';
import { ORIGINS } from './config.js';
import { pageFor } from './pages.js';
import './styles.css';

const root = document.getElementById('root');
if (!root) throw new Error('index.html has no #root');

// One build, three origins: the address decides which page this is.
const page = pageFor(window.location.origin, ORIGINS);

createRoot(root).render(
  <StrictMode>
    {page === 'login' ? (
      <LoginPage />
    ) : page === 'shop' ? (
      <ShopPage />
    ) : page === 'admin' ? (
      <AdminPage />
    ) : (
      <main>
        <h1>Unknown address</h1>
        <p role="alert" className="error">
          This build serves {ORIGINS.login}, {ORIGINS.shop} and {ORIGINS.admin}. It was opened at{' '}
          {window.location.origin}.
        </p>
      </main>
    )}
  </StrictMode>,
);
