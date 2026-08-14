// Generates the fixtures the histogram tests read:
//
//   histograms.root  every TH1/TH2 flavour allegro claims to support, written by
//                    ROOT itself
//   histograms.txt   the same histograms as ROOT reports them, which is what
//                    the Rust reader is compared against
//
// Regenerate with:  root -l -b -q tests/data/make_histograms.C
//
// The .root file is committed so the tests need no ROOT installation.

#include <cstdio>

static void describe(FILE *out, TH1 *h)
{
   const int nx = h->GetNbinsX();
   const int ny = h->GetDimension() == 2 ? h->GetNbinsY() : 0;
   fprintf(out, "histogram %s %s %d\n", h->GetName(), h->ClassName(), h->GetDimension());
   fprintf(out, "title %s\n", h->GetTitle());
   fprintf(out, "xtitle %s\n", h->GetXaxis()->GetTitle());
   fprintf(out, "ytitle %s\n", h->GetYaxis()->GetTitle());

   const TAxis *ax[2] = {h->GetXaxis(), h->GetYaxis()};
   for (int a = 0; a < (ny ? 2 : 1); ++a) {
      fprintf(out, "axis %c %d %.17g %.17g %d", a ? 'y' : 'x', ax[a]->GetNbins(), ax[a]->GetXmin(),
              ax[a]->GetXmax(), ax[a]->GetXbins()->GetSize() ? 1 : 0);
      for (int i = 0; i <= ax[a]->GetNbins(); ++i) fprintf(out, " %.17g", ax[a]->GetBinLowEdge(i + 1));
      fprintf(out, "\n");
   }

   fprintf(out, "stats %.17g %.17g %.17g %.17g\n", h->GetEntries(), h->Integral(), h->GetMean(),
           h->GetStdDev());
   fprintf(out, "sumw2 %d\n", h->GetSumw2N() ? 1 : 0);
   // Every cell, under- and overflows included, in ROOT's global numbering.
   for (int iy = 0; iy <= (ny ? ny + 1 : 0); ++iy)
      for (int ix = 0; ix <= nx + 1; ++ix) {
         const int bin = ny ? h->GetBin(ix, iy) : ix;
         fprintf(out, "cell %d %.17g %.17g\n", bin, h->GetBinContent(bin), h->GetBinError(bin));
      }
   fprintf(out, "end\n");
}

void make_histograms()
{
   TFile f("tests/data/histograms.root", "recreate");
   FILE *out = fopen("tests/data/histograms.txt", "w");
   fprintf(out, "# Written by tests/data/make_histograms.C under ROOT %s\n", gROOT->GetVersion());

   const double edges[6] = {0., 1., 3., 7., 10., 20.};

   // Uniform bins, double precision, explicit Sumw2 and axis titles: the
   // shape the four-muon analysis writes.
   TH1D h1d("h1d", "double, uniform", 5, 0., 5.);
   h1d.GetXaxis()->SetTitle("x [GeV]");
   h1d.GetYaxis()->SetTitle("Events");
   h1d.Sumw2();
   for (double x : {-1., 0.5, 0.5, 2.5, 4.9, 5.0, 99.}) h1d.Fill(x);

   // Variable bins, single precision.
   TH1F h1f("h1f", "float, variable", 5, edges);
   h1f.GetXaxis()->SetTitle("mass");
   for (double x : {-3., 0., 0.5, 1., 3., 9.9, 19.9, 20., 25.}) h1f.Fill(x);

   // Weighted fills, which make ROOT start tracking errors on its own.
   TH1D h1w("h1w", "double, weighted", 4, -2., 2.);
   h1w.Fill(0.5);
   h1w.Fill(0.5, 2.0);
   h1w.Fill(-1.5, 0.25);
   h1w.Fill(-9., 3.0);

   // The integer flavours, including saturation and truncated weights.
   TH1C h1c("h1c", "char", 3, 0., 3.);
   for (int i = 0; i < 200; ++i) h1c.Fill(0.5);
   h1c.Fill(1.5, 0.6);
   h1c.Fill(2.5, -300.);

   TH1S h1s("h1s", "short", 3, 0., 3.);
   h1s.Fill(0.5, 40000.);
   h1s.Fill(1.5, 12.);

   TH1I h1i("h1i", "int, variable", 5, edges);
   h1i.Fill(0.5, 3e9);
   h1i.Fill(2.5, 2.5);
   h1i.Fill(2.5, 2.5);

   TH1L h1l("h1l", "long", 3, 0., 3.);
   h1l.Fill(1.5, 5e18);
   h1l.Fill(0.5);

   // Two dimensions: uniform, variable in x, and an integer flavour.
   TH2D h2d("h2d", "double, 2-D", 3, 0., 3., 2, 0., 2.);
   h2d.GetXaxis()->SetTitle("x");
   h2d.GetYaxis()->SetTitle("y");
   h2d.GetZaxis()->SetTitle("Events");
   h2d.Sumw2();
   for (auto p : {std::make_pair(1.5, 0.5), {0.5, 1.5}, {1.5, 0.5}, {-1., 0.5}, {1.5, 9.}, {9., 9.}})
      h2d.Fill(p.first, p.second);

   TH2F h2f("h2f", "float, variable x", 5, edges, 3, -1., 2.);
   for (auto p : {std::make_pair(0.5, 0.), {8., 1.5}, {19.9, -0.5}, {25., 0.5}}) h2f.Fill(p.first, p.second);

   TH2I h2i("h2i", "int, 2-D, weighted", 2, 0., 2., 2, 0., 2.);
   h2i.Fill(0.5, 0.5, 3.0);
   h2i.Fill(1.5, 1.5, 2.5);

   TH1 *all[] = {&h1d, &h1f, &h1w, &h1c, &h1s, &h1i, &h1l, &h2d, &h2f, &h2i};
   for (TH1 *h : all) {
      h->Write();
      describe(out, h);
   }

   fclose(out);
   f.Close();
   printf("wrote tests/data/histograms.root and .txt\n");
}
