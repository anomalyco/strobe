// A custom-drawn window for strobe's image anchors and pixel waits (SPEC 9.6): its content is painted with
// GDI+ and exposes no accessibility tree, so only its pixels say what it shows. Input changes what it draws:
// a click on PLAY or Space counts (one bar each), a click on STOP or R resets, X shows or hides a square.
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

class Canvas : Form
{
    int count;
    bool square;
    readonly float scale;

    public Canvas()
    {
        Text = "Canvas fixture";
        scale = DeviceDpi / 96f;
        FormBorderStyle = FormBorderStyle.FixedSingle;
        ClientSize = new Size((int)(600 * scale), (int)(360 * scale));
        DoubleBuffered = true;
        BackColor = Color.FromArgb(32, 40, 48);
        KeyPreview = true;
    }

    static void At(Graphics g, Rectangle r, float s, Action<Graphics> draw)
    {
        var state = g.Save();
        g.ScaleTransform(s, s);
        g.TranslateTransform(r.X, r.Y);
        draw(g);
        g.Restore(state);
    }

    protected override void OnPaint(PaintEventArgs e)
    {
        var g = e.Graphics;
        Tiles.Setup(g);
        At(g, Tiles.Play, scale, Tiles.DrawPlay);
        At(g, Tiles.Stop, scale, Tiles.DrawStop);
        At(g, Tiles.ItemA, scale, Tiles.DrawItem);
        At(g, Tiles.ItemB, scale, Tiles.DrawItem);
        if (square) At(g, Tiles.Square, scale, Tiles.DrawSquare);
        for (int i = 0; i < count && i < 40; i++)
            g.FillRectangle(Brushes.White, (40 + i * 12) * scale, 270 * scale, 8 * scale, 40 * scale);
    }

    bool Hit(Rectangle r, Point p)
    {
        return p.X >= r.X * scale && p.Y >= r.Y * scale && p.X < r.Right * scale && p.Y < r.Bottom * scale;
    }

    protected override void OnMouseDown(MouseEventArgs e)
    {
        if (Hit(Tiles.Play, e.Location)) count++;
        else if (Hit(Tiles.Stop, e.Location)) count = 0;
        else return;
        Invalidate();
    }

    protected override void OnKeyDown(KeyEventArgs e)
    {
        if (e.KeyCode == Keys.Space) count++;
        else if (e.KeyCode == Keys.R) count = 0;
        else if (e.KeyCode == Keys.X) square = !square;
        else return;
        Invalidate();
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
        Application.Run(new Canvas());
        return 0;
    }
}
