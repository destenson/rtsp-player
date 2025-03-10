using System;
using System.Runtime.InteropServices;
using System.Windows.Forms;

namespace RtspPlayerNet
{
    public class RtspPlayer : IDisposable
    {
        // Native DLL imports
        private class NativeMethods
        {
            // Import the Rust compiled dll
            // Note: Ensure the correct calling convention (cdecl is typical for Rust FFI)
            [DllImport("rtsp_player_sys.dll", CallingConvention = CallingConvention.Cdecl)]
            public static extern IntPtr rtsp_player_create(string url);

            [DllImport("rtsp_player_ffi.dll", CallingConvention = CallingConvention.Cdecl)]
            public static extern bool rtsp_player_destroy(IntPtr handle);

            [DllImport("rtsp_player_ffi.dll", CallingConvention = CallingConvention.Cdecl)]
            public static extern bool rtsp_player_play(IntPtr handle);

            [DllImport("rtsp_player_ffi.dll", CallingConvention = CallingConvention.Cdecl)]
            public static extern bool rtsp_player_pause(IntPtr handle);

            [DllImport("rtsp_player_ffi.dll", CallingConvention = CallingConvention.Cdecl)]
            public static extern bool rtsp_player_stop(IntPtr handle);

            [DllImport("rtsp_player_ffi.dll", CallingConvention = CallingConvention.Cdecl)]
            public static extern bool rtsp_player_set_hwnd(IntPtr handle, IntPtr hwnd);

            [DllImport("rtsp_player_ffi.dll", CallingConvention = CallingConvention.Cdecl)]
            public static extern IntPtr rtsp_player_get_last_error();

            [DllImport("rtsp_player_ffi.dll", CallingConvention = CallingConvention.Cdecl)]
            public static extern void rtsp_player_free_string(IntPtr stringPtr);
        }

        private IntPtr _handle;
        private bool _disposed = false;

        /// <summary>
        /// Creates a new RTSP player instance
        /// </summary>
        /// <param name="url">The RTSP stream URL</param>
        public RtspPlayer(string url)
        {
            if (string.IsNullOrEmpty(url))
            {
                throw new ArgumentException("RTSP URL cannot be null or empty", nameof(url));
            }

            _handle = NativeMethods.rtsp_player_create(url);
            if (_handle == IntPtr.Zero)
            {
                string error = GetLastError();
                throw new InvalidOperationException($"Failed to create RTSP player: {error}");
            }
        }

        /// <summary>
        /// Attaches the video output to a Windows Forms control
        /// </summary>
        /// <param name="control">The control to render video to</param>
        public bool AttachToControl(Control control)
        {
            if (control == null)
                throw new ArgumentNullException(nameof(control));

            return NativeMethods.rtsp_player_set_hwnd(_handle, control.Handle);
        }

        /// <summary>
        /// Starts playing the RTSP stream
        /// </summary>
        public bool Play()
        {
            CheckDisposed();
            return NativeMethods.rtsp_player_play(_handle);
        }

        /// <summary>
        /// Pauses the RTSP stream
        /// </summary>
        public bool Pause()
        {
            CheckDisposed();
            return NativeMethods.rtsp_player_pause(_handle);
        }

        /// <summary>
        /// Stops the RTSP stream
        /// </summary>
        public bool Stop()
        {
            CheckDisposed();
            return NativeMethods.rtsp_player_stop(_handle);
        }

        /// <summary>
        /// Gets the last error message from the native player
        /// </summary>
        private string GetLastError()
        {
            IntPtr errorPtr = NativeMethods.rtsp_player_get_last_error();
            if (errorPtr == IntPtr.Zero)
                return "Unknown error";

            string errorMessage = Marshal.PtrToStringAnsi(errorPtr);
            NativeMethods.rtsp_player_free_string(errorPtr);
            return errorMessage;
        }

        private void CheckDisposed()
        {
            if (_disposed)
                throw new ObjectDisposedException(nameof(RtspPlayer));
        }

        #region IDisposable Implementation

        public void Dispose()
        {
            Dispose(true);
            GC.SuppressFinalize(this);
        }

        protected virtual void Dispose(bool disposing)
        {
            if (!_disposed)
            {
                if (_handle != IntPtr.Zero)
                {
                    // Stop playback on dispose
                    NativeMethods.rtsp_player_stop(_handle);
                    NativeMethods.rtsp_player_destroy(_handle);
                    _handle = IntPtr.Zero;
                }

                _disposed = true;
            }
        }

        ~RtspPlayer()
        {
            Dispose(false);
        }

        #endregion
    }
}
