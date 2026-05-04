package sh.sandboxed.dashboard

import android.os.Bundle
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.fragment.app.FragmentActivity
import sh.sandboxed.dashboard.ui.nav.AppRoot
import sh.sandboxed.dashboard.ui.theme.SandboxedTheme

class MainActivity : FragmentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        val container = (application as SandboxedDashboardApp).container
        setContent {
            SandboxedTheme {
                val settings by container.cached.collectAsState()
                AppRoot(container = container, settings = settings, host = this)
            }
        }
    }
}
