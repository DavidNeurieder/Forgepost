<script lang="ts">
	import { goto } from '$app/navigation';
	import { api, bootstrap, currentSession } from '$lib/api';

	let email = $state('');
	let password = $state('');
	let error = $state('');
	let busy = $state(false);
	let checking = $state(true);

	$effect(() => {
		api<{ setup_complete: boolean }>('/setup')
			.then((s) => {
				if (!s.setup_complete) return goto('/setup');
				return currentSession().then(() => goto('/admin'));
			})
			.catch(() => {})
			.finally(() => (checking = false));
	});

	async function submit() {
		error = '';
		busy = true;
		try {
			await bootstrap('/login', { email: email.trim(), password });
			goto('/admin');
		} catch (e) {
			error = e instanceof Error ? e.message : 'Login failed.';
		} finally {
			busy = false;
		}
	}
</script>

<svelte:head>
	<title>Log in · OpenPublish</title>
</svelte:head>

<h1>Log in</h1>

{#if checking}
	<p class="muted">Checking…</p>
{:else}
	<form
		onsubmit={(e) => {
			e.preventDefault();
			submit();
		}}
	>
		<field>
			<label for="email">Email</label>
			<input id="email" type="email" bind:value={email} autocomplete="email" />
		</field>
		<field>
			<label for="password">Password</label>
			<input id="password" type="password" bind:value={password} autocomplete="current-password" />
		</field>
		{#if error}
			<p class="error">{error}</p>
		{/if}
		<button type="submit" disabled={busy}>Log in</button>
	</form>
{/if}
