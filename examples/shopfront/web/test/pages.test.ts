import { describe, expect, it } from 'vitest';
import { allowedReturn, pageFor, type Origins } from '../src/pages.js';

const local: Origins = {
  login: 'http://localhost:5180',
  shop: 'http://localhost:5181',
  admin: 'http://localhost:5182',
};

const live: Origins = {
  login: 'https://login.example.com',
  shop: 'https://shop.example.com',
  admin: 'https://admin.example.com',
};

describe('the page chosen for an origin', () => {
  it('picks login, shop and admin by origin', () => {
    expect(pageFor('http://localhost:5180', local)).toBe('login');
    expect(pageFor('http://localhost:5181', local)).toBe('shop');
    expect(pageFor('http://localhost:5182', local)).toBe('admin');
    expect(pageFor('https://login.example.com', live)).toBe('login');
    expect(pageFor('https://shop.example.com', live)).toBe('shop');
    expect(pageFor('https://admin.example.com', live)).toBe('admin');
  });

  it('ignores a trailing slash or path in the settings', () => {
    expect(pageFor('https://shop.example.com', { ...live, shop: 'https://shop.example.com/' })).toBe('shop');
  });

  it('picks nothing for another origin, another port or another scheme', () => {
    expect(pageFor('http://localhost:5173', local)).toBeNull();
    expect(pageFor('https://localhost:5181', local)).toBeNull();
    expect(pageFor('https://evil.example.com', live)).toBeNull();
    expect(pageFor('not an address', live)).toBeNull();
  });
});

describe('the login page return_to check', () => {
  it('accepts the shop and the console with any path and query', () => {
    expect(allowedReturn('https://shop.example.com/', live)).toBe(true);
    expect(allowedReturn('https://shop.example.com/cart?x=1#top', live)).toBe(true);
    expect(allowedReturn('https://admin.example.com/orders/7', live)).toBe(true);
    expect(allowedReturn('http://localhost:5181/anything', local)).toBe(true);
  });

  it('refuses the login page itself and every other origin', () => {
    expect(allowedReturn('https://login.example.com/', live)).toBe(false);
    expect(allowedReturn('https://evil.example.com/', live)).toBe(false);
    expect(allowedReturn('https://shop.example.com.evil.com/', live)).toBe(false);
    expect(allowedReturn('http://shop.example.com/', live)).toBe(false);
    expect(allowedReturn('https://shop.example.com:8443/', live)).toBe(false);
  });

  it('refuses what is not an address', () => {
    expect(allowedReturn('', live)).toBe(false);
    expect(allowedReturn('/relative', live)).toBe(false);
    expect(allowedReturn('javascript:alert(1)', live)).toBe(false);
  });
});
