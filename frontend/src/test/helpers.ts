import { tick } from 'svelte';

/**
 * Poll `assertion` while flushing Svelte's update queue on every iteration.
 * Component tests mutate `$state` from resolved promises; Svelte 5 schedules
 * those DOM updates on a microtask that `@testing-library`'s `waitFor` does not
 * trigger, so we interleave `tick()` with real-timer macrotask turns.
 */
export async function waitForFlush(assertion: () => void, timeoutMs = 3000): Promise<void> {
	const deadline = Date.now() + timeoutMs;
	let lastError: unknown;
	while (Date.now() < deadline) {
		await tick();
		try {
			assertion();
			return;
		} catch (error) {
			lastError = error;
		}
		await new Promise((resolve) => setTimeout(resolve, 20));
	}
	throw lastError;
}
