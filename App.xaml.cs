using System;
using System.Collections.Generic;
using System.IO;
using System.Reflection;
using System.Threading;
using System.Windows;

namespace PulseGlassWidget
{
    public partial class App : Application
    {
        private static Mutex _singleInstanceMutex;
        private static bool _ownsMutex;
        private static readonly Dictionary<string, Assembly> _embeddedAssemblies =
            new Dictionary<string, Assembly>(StringComparer.OrdinalIgnoreCase);

        static App()
        {
            AppDomain.CurrentDomain.AssemblyResolve += ResolveEmbeddedAssembly;
        }

        // The self-updater only replaces the exe, so libraries ship inside it as resources.
        private static Assembly ResolveEmbeddedAssembly(object sender, ResolveEventArgs args)
        {
            string name = new AssemblyName(args.Name).Name;
            lock (_embeddedAssemblies)
            {
                Assembly asm;
                if (_embeddedAssemblies.TryGetValue(name, out asm)) return asm;
                using (var stream = typeof(App).Assembly.GetManifestResourceStream("Libs." + name + ".dll"))
                {
                    if (stream == null) return null;
                    var ms = new MemoryStream();
                    stream.CopyTo(ms);
                    asm = Assembly.Load(ms.ToArray());
                }
                _embeddedAssemblies[name] = asm;
                return asm;
            }
        }

        // Older releases put LibreHardwareMonitorLib.dll next to the exe. The loader finds that
        // file before asking ResolveEmbeddedAssembly, so an exe-only update would keep the old library.
        private static void DeleteStaleLibraries()
        {
            try
            {
                string path = Path.Combine(Path.GetDirectoryName(typeof(App).Assembly.Location), "LibreHardwareMonitorLib.dll");
                if (File.Exists(path)) File.Delete(path);
            }
            catch { }
        }

        public static void ReleaseSingleInstanceMutex()
        {
            if (_singleInstanceMutex != null)
            {
                if (_ownsMutex)
                {
                    try { _singleInstanceMutex.ReleaseMutex(); } catch { }
                    _ownsMutex = false;
                }
                try { _singleInstanceMutex.Dispose(); } catch { }
                _singleInstanceMutex = null;
            }
        }

        protected override void OnStartup(StartupEventArgs e)
        {
            bool createdNew;
            _singleInstanceMutex = new Mutex(true, "PulseGlassWidget_SingleInstance_Mutex", out createdNew);
            if (!createdNew)
            {
                // Brief wait in case the previous non-elevated instance is handing off after UAC elevation
                try { createdNew = _singleInstanceMutex.WaitOne(1500); } catch { createdNew = false; }
            }
            _ownsMutex = createdNew;
            if (!createdNew)
            {
                MessageBox.Show("PulseGlass zaten çalışıyor! Sistem tepsisinden (sağ alt) açabilirsiniz.",
                    "PulseGlass", MessageBoxButton.OK, MessageBoxImage.Information);
                Shutdown();
                return;
            }

            DeleteStaleLibraries();
            base.OnStartup(e);
            DispatcherUnhandledException += (s, args) =>
            {
                try
                {
                    File.WriteAllText(
                        Path.Combine(AppDomain.CurrentDomain.BaseDirectory, "startup_error.log"),
                        args.Exception.ToString());
                }
                catch { }
                MessageBox.Show("Uygulama başlatılırken hata oluştu:\n" + args.Exception.Message,
                    "PulseGlass Hata", MessageBoxButton.OK, MessageBoxImage.Error);
                args.Handled = true;
                Shutdown();
            };
        }

        protected override void OnExit(ExitEventArgs e)
        {
            ReleaseSingleInstanceMutex();
            base.OnExit(e);
        }
    }
}
