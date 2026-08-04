<script lang="ts">
	import favicon from '$lib/assets/favicon.svg';
	import '../app.css';

	let { children } = $props();
</script>

<svelte:head>
	<link rel="icon" href={favicon} />
	<meta name="viewport" content="width=device-width, initial-scale=1" />
	<title>OpenPublish</title>
</svelte:head>

<header class="site">
	<div class="container">
		<a class="brand" href="/">OpenPublish</a>
		<nav class="site">
			<a href="/">Blog</a>
			<a href="/admin">Admin</a>
		</nav>
	</div>
</header>

<main class="container">
	{@render children()}
</main>
