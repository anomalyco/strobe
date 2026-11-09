// A custom-drawn window for strobe's image anchors and pixel waits (SPEC 9.6): its content is drawn with
// GDI+ into a bitmap and exposes no accessibility tree, so only its pixels say what it shows. It shows each
// frame through a DXGI flip-model swap chain (Direct3D 11, two buffers), so the OS's present events see
// every frame it draws; `--gdi` paints the window with GDI instead, which makes no present event.
// Input changes what it draws: a click on PLAY or Space counts (one bar each), a click on STOP or R resets,
// X shows or hides a square, PRESS lights or darkens on the button's press and RELEASE on its release (R
// darkens both).
//
// Build (no SDK needed, the .NET Framework's compiler ships with Windows):
//   C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe /nologo /target:winexe /out:<dir>\canvas-app.exe canvas-app.cs
// Anchors: canvas-app.exe --anchors <dir> writes each tile as a PNG (1x, and PLAY at 1.5x and 2x) and exits.
using System;
using System.Drawing;
using System.Drawing.Drawing2D;
using System.Drawing.Imaging;
using System.Drawing.Text;
using System.IO;
using System.Runtime.InteropServices;
using System.Windows.Forms;

static class Tiles
{
    public static readonly Rectangle Play = new Rectangle(40, 40, 160, 60);
    public static readonly Rectangle Stop = new Rectangle(240, 40, 160, 60);
    public static readonly Rectangle ItemA = new Rectangle(40, 160, 120, 50);
    public static readonly Rectangle ItemB = new Rectangle(240, 160, 120, 50);
    public static readonly Rectangle Square = new Rectangle(440, 160, 100, 100);
    public static readonly Rectangle Press = new Rectangle(40, 330, 160, 60);
    public static readonly Rectangle Release = new Rectangle(240, 330, 160, 60);

    static void Text(Graphics g, string s, float x, float y, Color c)
    {
        using (var f = new Font("Arial", 14f, FontStyle.Bold, GraphicsUnit.Pixel))
        using (var b = new SolidBrush(c))
            g.DrawString(s, f, b, x, y);
    }

    // Each tile at its own origin, in device-independent pixels.
    public static void DrawPlay(Graphics g)
    {
        g.FillRectangle(new SolidBrush(Color.FromArgb(40, 160, 80)), 0, 0, 160, 60);
        g.DrawRectangle(new Pen(Color.White, 3), 6, 6, 148, 48);
        g.FillPolygon(new SolidBrush(Color.FromArgb(16, 48, 16)), new[] { new PointF(24, 16), new PointF(24, 44), new PointF(48, 30) });
        Text(g, "PLAY", 70, 20, Color.White);
    }

    public static void DrawStop(Graphics g)
    {
        g.FillRectangle(new SolidBrush(Color.FromArgb(190, 50, 50)), 0, 0, 160, 60);
        g.FillRectangle(new SolidBrush(Color.FromArgb(48, 16, 16)), 22, 16, 28, 28);
        Text(g, "STOP", 70, 20, Color.White);
    }

    public static void DrawItem(Graphics g)
    {
        g.FillRectangle(new SolidBrush(Color.FromArgb(40, 80, 190)), 0, 0, 120, 50);
        g.FillEllipse(new SolidBrush(Color.FromArgb(240, 210, 40)), 45, 10, 30, 30);
    }

    public static void DrawSquare(Graphics g)
    {
        g.FillRectangle(new SolidBrush(Color.FromArgb(240, 140, 30)), 0, 0, 100, 100);
        g.FillPolygon(Brushes.Black, new[] { new PointF(50, 15), new PointF(85, 50), new PointF(50, 85), new PointF(15, 50) });
    }

    // A tile that lights up: dark grey, or amber when lit.
    public static Action<Graphics> Lamp(string label, bool lit)
    {
        return g =>
        {
            using (var fill = new SolidBrush(lit ? Color.FromArgb(250, 180, 40) : Color.FromArgb(70, 76, 84)))
                g.FillRectangle(fill, 0, 0, 160, 60);
            Text(g, label, 30, 20, lit ? Color.Black : Color.White);
        };
    }

    // Not drawn anywhere: an anchor that is never found.
    public static void DrawMissing(Graphics g)
    {
        g.FillRectangle(new SolidBrush(Color.FromArgb(150, 40, 170)), 0, 0, 80, 40);
        g.FillEllipse(Brushes.White, 25, 5, 30, 30);
    }

    public static void Setup(Graphics g)
    {
        g.SmoothingMode = SmoothingMode.AntiAlias;
        g.TextRenderingHint = TextRenderingHint.AntiAliasGridFit;
    }

    // `draw` at `scale` into a PNG of the tile's size.
    public static void Save(string path, int w, int h, float scale, Action<Graphics> draw)
    {
        using (var bmp = new Bitmap((int)Math.Round(w * scale), (int)Math.Round(h * scale), PixelFormat.Format32bppArgb))
        using (var g = Graphics.FromImage(bmp))
        {
            Setup(g);
            g.ScaleTransform(scale, scale);
            draw(g);
            bmp.Save(path, ImageFormat.Png);
        }
    }
}

// A Direct3D 11 device and a DXGI flip-model swap chain on a window, called through their COM vtables
// (no SDK and no managed wrapper): each frame is a bitmap copied into the back buffer and presented.
sealed class Presenter
{
    [DllImport("d3d11.dll")]
    static extern int D3D11CreateDevice(IntPtr adapter, int driverType, IntPtr software, uint flags, IntPtr levels, uint levelCount, uint sdkVersion, out IntPtr device, out int level, out IntPtr context);

    [StructLayout(LayoutKind.Sequential)]
    struct SwapChainDesc1
    {
        public uint Width, Height;
        public int Format;
        public int Stereo;
        public uint SampleCount, SampleQuality;
        public uint BufferUsage, BufferCount;
        public int Scaling, SwapEffect, AlphaMode;
        public uint Flags;
    }

    delegate int QueryInterfaceFn(IntPtr self, ref Guid iid, out IntPtr obj);
    delegate int GetAdapterFn(IntPtr self, out IntPtr adapter);
    delegate int GetParentFn(IntPtr self, ref Guid iid, out IntPtr parent);
    delegate int CreateSwapChainForHwndFn(IntPtr self, IntPtr device, IntPtr hwnd, ref SwapChainDesc1 desc, IntPtr fullscreen, IntPtr output, out IntPtr swapChain);
    delegate int GetBufferFn(IntPtr self, uint index, ref Guid iid, out IntPtr surface);
    delegate int PresentFn(IntPtr self, uint syncInterval, uint flags);
    delegate void UpdateSubresourceFn(IntPtr self, IntPtr resource, uint subresource, IntPtr box, IntPtr data, uint rowPitch, uint depthPitch);

    static Guid IDXGIDevice = new Guid("54ec77fa-1377-44e6-8c32-88fd5f44c84c");
    static Guid IDXGIFactory2 = new Guid("50c83a1c-e072-4c48-87b0-3630fa36a6d0");
    static Guid ID3D11Texture2D = new Guid("6f15aaf2-d208-4e89-9ab4-489535d34f9c");

    // Vtable slots: IDXGIDevice::GetAdapter 7, IDXGIObject::GetParent 6, IDXGIFactory2::CreateSwapChainForHwnd 15,
    // IDXGISwapChain::Present 8 and GetBuffer 9, ID3D11DeviceContext::UpdateSubresource 48.
    static T Slot<T>(IntPtr obj, int slot) where T : class
    {
        var vtable = Marshal.ReadIntPtr(obj);
        return (T)(object)Marshal.GetDelegateForFunctionPointer(Marshal.ReadIntPtr(vtable, slot * IntPtr.Size), typeof(T));
    }

    static void Check(int hr, string what)
    {
        if (hr < 0) throw new InvalidOperationException(string.Format("{0}: HRESULT 0x{1:X8}", what, hr));
    }

    readonly IntPtr context, swapChain, backBuffer;
    readonly PresentFn present;
    readonly UpdateSubresourceFn update;

    public Presenter(IntPtr hwnd, int width, int height)
    {
        IntPtr device;
        int level;
        // A hardware device (D3D_DRIVER_TYPE_HARDWARE) with BGRA support, the SDK's version 7.
        Check(D3D11CreateDevice(IntPtr.Zero, 1, IntPtr.Zero, 0x20, IntPtr.Zero, 0, 7, out device, out level, out context), "D3D11CreateDevice");
        // The factory that made the device makes its swap chain.
        IntPtr dxgiDevice, adapter, factory;
        Check(Slot<QueryInterfaceFn>(device, 0)(device, ref IDXGIDevice, out dxgiDevice), "IDXGIDevice");
        Check(Slot<GetAdapterFn>(dxgiDevice, 7)(dxgiDevice, out adapter), "GetAdapter");
        Check(Slot<GetParentFn>(adapter, 6)(adapter, ref IDXGIFactory2, out factory), "IDXGIFactory2");
        // BGRA (DXGI_FORMAT_B8G8R8A8_UNORM), a render target output, two buffers, DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL.
        var desc = new SwapChainDesc1 { Width = (uint)width, Height = (uint)height, Format = 87, SampleCount = 1, BufferUsage = 0x20, BufferCount = 2, SwapEffect = 3 };
        Check(Slot<CreateSwapChainForHwndFn>(factory, 15)(factory, device, hwnd, ref desc, IntPtr.Zero, IntPtr.Zero, out swapChain), "CreateSwapChainForHwnd");
        // With a flip model swap chain, Direct3D 11's buffer 0 is always the one to draw next.
        Check(Slot<GetBufferFn>(swapChain, 9)(swapChain, 0, ref ID3D11Texture2D, out backBuffer), "GetBuffer");
        present = Slot<PresentFn>(swapChain, 8);
        update = Slot<UpdateSubresourceFn>(context, 48);
    }

    // The bitmap (32bppArgb: BGRA in memory, as the swap chain's format) on the screen at the next refresh.
    public void Show(Bitmap frame)
    {
        var data = frame.LockBits(new Rectangle(0, 0, frame.Width, frame.Height), ImageLockMode.ReadOnly, PixelFormat.Format32bppArgb);
        try
        {
            update(context, backBuffer, 0, IntPtr.Zero, data.Scan0, (uint)data.Stride, 0);
        }
        finally
        {
            frame.UnlockBits(data);
        }
        Check(present(swapChain, 1, 0), "Present");
    }
}

class Canvas : Form
{
    int count;
    bool square, pressLit, releaseLit;
    readonly float scale;
    readonly bool gdi;
    Presenter presenter;
    Bitmap frame;

    [DllImport("user32.dll")]
    static extern uint GetDpiForWindow(IntPtr hwnd);

    public Canvas(bool gdi)
    {
        this.gdi = gdi;
        Text = gdi ? "Canvas fixture (GDI)" : "Canvas fixture";
        // The window's style is set before its handle exists: a later change would make a new window.
        FormBorderStyle = FormBorderStyle.FixedSingle;
        MaximizeBox = false;
        BackColor = Color.FromArgb(32, 40, 48);
        KeyPreview = true;
        if (gdi) DoubleBuffered = true;
        else SetStyle(ControlStyles.AllPaintingInWmPaint | ControlStyles.UserPaint | ControlStyles.Opaque, true);
        // The window's own DPI: the .NET Framework's DeviceDpi stays 96 in a per-monitor aware process.
        scale = GetDpiForWindow(Handle) / 96f;
        ClientSize = new Size((int)Math.Round(600 * scale), (int)Math.Round(420 * scale));
    }

    protected override void OnLoad(EventArgs e)
    {
        base.OnLoad(e);
        if (gdi) return;
        frame = new Bitmap(ClientSize.Width, ClientSize.Height, PixelFormat.Format32bppArgb);
        presenter = new Presenter(Handle, ClientSize.Width, ClientSize.Height);
        Render();
    }

    static void At(Graphics g, Rectangle r, float s, Action<Graphics> draw)
    {
        var state = g.Save();
        g.ScaleTransform(s, s);
        g.TranslateTransform(r.X, r.Y);
        draw(g);
        g.Restore(state);
    }

    void Draw(Graphics g)
    {
        g.Clear(BackColor);
        Tiles.Setup(g);
        At(g, Tiles.Play, scale, Tiles.DrawPlay);
        At(g, Tiles.Stop, scale, Tiles.DrawStop);
        At(g, Tiles.ItemA, scale, Tiles.DrawItem);
        At(g, Tiles.ItemB, scale, Tiles.DrawItem);
        At(g, Tiles.Press, scale, Tiles.Lamp("PRESS", pressLit));
        At(g, Tiles.Release, scale, Tiles.Lamp("RELEASE", releaseLit));
        if (square) At(g, Tiles.Square, scale, Tiles.DrawSquare);
        for (int i = 0; i < count && i < 40; i++)
            g.FillRectangle(Brushes.White, (40 + i * 12) * scale, 270 * scale, 8 * scale, 40 * scale);
    }

    // One frame: drawn into the bitmap and presented.
    void Render()
    {
        using (var g = Graphics.FromImage(frame))
            Draw(g);
        presenter.Show(frame);
    }

    void Changed()
    {
        if (gdi) Invalidate();
        else Render();
    }

    protected override void OnPaint(PaintEventArgs e)
    {
        if (gdi) Draw(e.Graphics);
        else if (presenter != null) Render();
    }

    bool Hit(Rectangle r, Point p)
    {
        return p.X >= r.X * scale && p.Y >= r.Y * scale && p.X < r.Right * scale && p.Y < r.Bottom * scale;
    }

    protected override void OnMouseDown(MouseEventArgs e)
    {
        if (Hit(Tiles.Play, e.Location)) count++;
        else if (Hit(Tiles.Stop, e.Location)) count = 0;
        else if (Hit(Tiles.Press, e.Location)) pressLit = !pressLit;
        else return;
        Changed();
    }

    protected override void OnMouseUp(MouseEventArgs e)
    {
        if (!Hit(Tiles.Release, e.Location)) return;
        releaseLit = !releaseLit;
        Changed();
    }

    protected override void OnKeyDown(KeyEventArgs e)
    {
        if (e.KeyCode == Keys.Space) count++;
        else if (e.KeyCode == Keys.R) count = 0;
        else if (e.KeyCode == Keys.X) square = !square;
        else return;
        if (e.KeyCode == Keys.R) pressLit = releaseLit = false;
        Changed();
    }
}

static class Program
{
    [DllImport("user32.dll")]
    static extern bool SetProcessDpiAwarenessContext(IntPtr value);

    [STAThread]
    static int Main(string[] args)
    {
        if (args.Length == 2 && args[0] == "--anchors")
        {
            var dir = args[1];
            Directory.CreateDirectory(dir);
            Tiles.Save(Path.Combine(dir, "play.png"), 160, 60, 1f, Tiles.DrawPlay);
            Tiles.Save(Path.Combine(dir, "play@1.5x.png"), 160, 60, 1.5f, Tiles.DrawPlay);
            Tiles.Save(Path.Combine(dir, "play@2x.png"), 160, 60, 2f, Tiles.DrawPlay);
            Tiles.Save(Path.Combine(dir, "stop.png"), 160, 60, 1f, Tiles.DrawStop);
            Tiles.Save(Path.Combine(dir, "item.png"), 120, 50, 1f, Tiles.DrawItem);
            Tiles.Save(Path.Combine(dir, "square.png"), 100, 100, 1f, Tiles.DrawSquare);
            Tiles.Save(Path.Combine(dir, "missing.png"), 80, 40, 1f, Tiles.DrawMissing);
            return 0;
        }
        // Per-monitor aware: drawn at the display's own scale, never stretched by the OS.
        SetProcessDpiAwarenessContext(new IntPtr(-4));
        Application.EnableVisualStyles();
        Application.Run(new Canvas(Array.IndexOf(args, "--gdi") >= 0));
        return 0;
    }
}
