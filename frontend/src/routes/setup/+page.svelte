<script lang="ts">
	import { goto } from '$app/navigation';
	import { api, bootstrap } from '$lib/api';

	let checking = $state(true);
	let email = $state('');
	let displayName = $state('');
	let password = $state('');
	let confirm = $state('');
	let error = $state('');
	let busy = $state(false);

	$effect(() => {
		api<{ setup_complete: boolean }>('/setup')
			.then((s) => {
				if (s.setup_complete) goto('/login');
			})
			.catch(() => {})
			.finally(() => (checking = false));
	});

	async function submit() {
		error = '';
		if (!email.includes('@')) {
			error = 'Enter a valid email address.';
			return;
		}
		if (password.length < 8) {
			error = 'Password must be at least 8 characters.';
			return;
		}
		if (password !== confirm) {
			error = 'Passwords do not match.';
			return;
		}
		if (!displayName.trim()) {
			error = 'Enter a display name.';
			return;
		}
		busy = true;
		try {
			await bootstrap('/setup', {
				email: email.trim(),
				password,
				display_name: displayName.trim()
			});
			goto('/admin');
		} catch (e) {
			error = e instanceof Error ? e.message : 'Setup failed.';
		} finally {
			busy = false;
		}
	}
</script>

<svelte:head>
	<title>Set up · OpenPublish</title>
</svelte:head>

<h1>Welcome to OpenPublish</h1>
<p class="muted">
	Create the owner account. You will be the only administrator of this blog.
</p>

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
			<label for="display">Display name</label>
			<input id="display" bind:value={displayName} autocomplete="name" />
		</field>
		<field>
			<label for="password">Password</label>
			<input id="password" type="password" bind:value={password} autocomplete="new-password" />
		</field>
		<field>
			<label for="confirm">Confirm password</label>
			<input id="confirm" type="password" bind:value={confirm} autocomplete="new-password" />
		</field>
		{#if error}
			<p class="error">{error}</p>
		{/if}
		<button type="submit" disabled={busy}>Create account</button>
	</form>
{/if}
