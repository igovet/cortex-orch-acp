package __PACKAGE__

import android.os.Bundle
import android.view.View
import androidx.activity.enableEdgeToEdge
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat

// The build script installs this file over the generated activity (the Android project itself is
// generated on every build and never kept).
class MainActivity : TauriActivity() {
  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)
    // Drawing edge to edge, the system no longer resizes the page for the on-screen keyboard. Give
    // the page exactly the space above it, so a composer or a form's button is never under the keys.
    // The status and navigation bars stay the page's own business (its safe-area insets).
    val content = findViewById<View>(android.R.id.content)
    ViewCompat.setOnApplyWindowInsetsListener(content) { view, insets ->
      view.setPadding(0, 0, 0, insets.getInsets(WindowInsetsCompat.Type.ime()).bottom)
      insets
    }
  }
}
