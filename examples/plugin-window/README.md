# Plugin-owned canvas

This example uses the shared UI controls and opens its own GPU window from a
small panel in XRTranslate. Animation, drag input, pause and full screen belong
to the plugin. Closing the window hides it; the host panel can reopen it.
Disabling the plugin or closing XRTranslate also closes the window.

Build it with the SDK, without building or checking out the host:

```powershell
C:/SDK/XRTranslate/xrt-plugin.exe build . --sdk C:/SDK/XRTranslate --output C:/Packages/canvas-example
```

Copy the package into the application's `plugins/` directory, then choose **Open
canvas** on its page. For development, run the executable with `--standalone`.
The optional `--director-port <port>` enables the shared UI automation tools for
that window; it is not enabled by default.

No window images or drawing commands are sent to XRTranslate. The same
`Context::client()` used by other plugins can submit tasks to the host. An engine
other than the one in this example can own the rendering loop as well.
