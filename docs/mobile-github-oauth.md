# Mobile GitHub OAuth

The mobile clients should expose GitHub as an account connection under Settings.
Users should not paste GitHub tokens or set runtime environment variables in the
app.

## Flow

1. Fetch status:
   ```http
   GET /api/auth/github/status
   Authorization: Bearer <dashboard-jwt>
   ```

2. Start OAuth:
   ```http
   POST /api/auth/github/authorize
   Authorization: Bearer <dashboard-jwt>
   ```

   The response contains a GitHub authorization `url`.

3. Open `url` in the system browser:
   - iOS: `openURL(url)`
   - Android: Chrome Custom Tabs or `Intent.ACTION_VIEW`

4. GitHub redirects back to the sandboxed.sh server callback. The server stores
   the OAuth token in the encrypted secrets store for the current sandbox user.

5. When the app becomes active again, refresh `GET /api/auth/github/status`.

6. Disconnect:
   ```http
   DELETE /api/auth/github
   Authorization: Bearer <dashboard-jwt>
   ```

## Android Sketch

```kotlin
suspend fun connectGithub(api: SandboxedApi, context: Context) {
    val auth = api.startGithubOAuth()
    CustomTabsIntent.Builder()
        .build()
        .launchUrl(context, Uri.parse(auth.url))
}

override fun onResume() {
    super.onResume()
    lifecycleScope.launch {
        githubStatus = api.getGithubOAuthStatus()
    }
}
```

The server must be configured with `GITHUB_OAUTH_CLIENT_ID`,
`GITHUB_OAUTH_CLIENT_SECRET`, and an unlocked secrets store. Mobile clients only
display the resulting connection state and account metadata.
