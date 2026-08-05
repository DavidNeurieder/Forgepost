import '@testing-library/jest-dom/vitest';
import { cleanup } from '@testing-library/svelte';
import { afterEach } from 'vitest';

// jsdom does not implement Web Storage; api.ts uses `localStorage` for the
// CSRF token, so install a tiny in-memory stand-in.
class MemoryStorage implements Storage {
	private store = new Map<string, string>();

	get length() {
		return this.store.size;
	}

	clear() {
		this.store.clear();
	}

	getItem(key: string) {
		return this.store.has(key) ? this.store.get(key)! : null;
	}

	key(index: number) {
		return [...this.store.keys()][index] ?? null;
	}

	removeItem(key: string) {
		this.store.delete(key);
	}

	setItem(key: string, value: string) {
		this.store.set(key, String(value));
	}
}

if (typeof window !== 'undefined' && !window.localStorage) {
	const storage = new MemoryStorage();
	Object.defineProperty(window, 'localStorage', { value: storage, configurable: true });
	Object.defineProperty(globalThis, 'localStorage', { value: storage, configurable: true });
}

afterEach(() => cleanup());
