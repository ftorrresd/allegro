#include <ROOT/RVec.hxx>
#include <TCanvas.h>
#include <TFile.h>
#include <TH1D.h>
#include <TLorentzVector.h>
#include <TNamed.h>
#include <TStyle.h>
#include <TTree.h>
#include <TTreeReader.h>
#include <TTreeReaderArray.h>
#include <TTreeReaderValue.h>

#include <algorithm>
#include <array>
#include <cmath>
#include <iostream>
#include <limits>
#include <stdexcept>
#include <vector>

void fourMuonMass(
    const char *input =
        "root://cms-xrd-global.cern.ch//store/mc/RunIII2024Summer24NanoAODv15/"
        "GluGluHto2Zto4Mu-newPS_Bin-M4L-70_Fil-4L_Par-g1-g1prime2-M-125-Ga-SM_"
        "TuneCP5_13p6TeV_mcfm-pythia8/NANOAODSIM/150X_mcRun3_2024_realistic_v2-v2/"
        "100000/f58c7d36-3436-4ae3-b867-eb19b609280d.root",
    const char *output = "fourMuonMass.root") {
  TFile *source = TFile::Open(input, "READ");
  if (!source || source->IsZombie())
    throw std::runtime_error("Could not open the input through XRootD");

  TTree *events = nullptr;
  source->GetObject("Events", events);
  if (!events)
    throw std::runtime_error("The input has no Events tree");

  TTreeReader reader(events);
  TTreeReaderValue<Bool_t> trigger(
      reader, "HLT_Mu17_TrkIsoVVL_Mu8_TrkIsoVVL_DZ_Mass3p8");
  TTreeReaderValue<UInt_t> run(reader, "run");
  TTreeReaderValue<UInt_t> luminosityBlock(reader, "luminosityBlock");
  TTreeReaderValue<ULong64_t> event(reader, "event");
  TTreeReaderArray<Float_t> muonPt(reader, "Muon_pt");
  TTreeReaderArray<Float_t> muonEta(reader, "Muon_eta");
  TTreeReaderArray<Float_t> muonPhi(reader, "Muon_phi");
  TTreeReaderArray<Float_t> muonMass(reader, "Muon_mass");
  TTreeReaderArray<Int_t> muonCharge(reader, "Muon_charge");
  TTreeReaderArray<Bool_t> muonMediumId(reader, "Muon_mediumId");
  TTreeReaderArray<Float_t> muonIso(reader, "Muon_pfRelIso04_all");

  TFile result(output, "RECREATE");
  TH1D massHistogram("hFourMuonMass",
                     "H #rightarrow ZZ^{*} #rightarrow 4#mu;m_{4#mu} [GeV];Events",
                     110, 70., 180.);
  massHistogram.Sumw2();

  TTree selectedEvents("FourMuonTree", "Selected four-muon candidates");
  UInt_t outRun = 0, outLumi = 0;
  ULong64_t outEvent = 0;
  Float_t fourMuonMassValue = 0., z1Mass = 0., z2Mass = 0.;
  Float_t pt[4] = {}, eta[4] = {}, phi[4] = {};
  Int_t charge[4] = {};
  selectedEvents.Branch("run", &outRun, "run/i");
  selectedEvents.Branch("luminosityBlock", &outLumi, "luminosityBlock/i");
  selectedEvents.Branch("event", &outEvent, "event/l");
  selectedEvents.Branch("fourMuonMass", &fourMuonMassValue, "fourMuonMass/F");
  selectedEvents.Branch("z1Mass", &z1Mass, "z1Mass/F");
  selectedEvents.Branch("z2Mass", &z2Mass, "z2Mass/F");
  selectedEvents.Branch("muonPt", pt, "muonPt[4]/F");
  selectedEvents.Branch("muonEta", eta, "muonEta[4]/F");
  selectedEvents.Branch("muonPhi", phi, "muonPhi[4]/F");
  selectedEvents.Branch("muonCharge", charge, "muonCharge[4]/I");

  Long64_t triggered = 0;
  while (reader.Next()) {
    if (!*trigger)
      continue;
    ++triggered;

    std::vector<unsigned int> goodMuons;
    for (unsigned int i = 0; i < muonPt.GetSize(); ++i) {
      if (muonPt[i] > 5. && std::abs(muonEta[i]) < 2.4 && muonMediumId[i] &&
          muonIso[i] < 0.4)
        goodMuons.push_back(i);
    }
    if (goodMuons.size() < 4)
      continue;

    std::array<unsigned int, 4> bestIndices{};
    float bestZ1 = 0., bestZ2 = 0.;
    double bestScore = std::numeric_limits<double>::max();
    bool found = false;

    for (size_t a = 0; a + 3 < goodMuons.size(); ++a)
      for (size_t b = a + 1; b + 2 < goodMuons.size(); ++b)
        for (size_t c = b + 1; c + 1 < goodMuons.size(); ++c)
          for (size_t d = c + 1; d < goodMuons.size(); ++d) {
            std::array<unsigned int, 4> idx = {
                goodMuons[a], goodMuons[b], goodMuons[c], goodMuons[d]};
            std::sort(idx.begin(), idx.end(), [&](unsigned int x, unsigned int y) {
              return muonPt[x] > muonPt[y];
            });
            if (muonPt[idx[0]] <= 20. || muonPt[idx[1]] <= 10.)
              continue;
            int totalCharge = 0;
            std::array<TLorentzVector, 4> p4;
            for (int i = 0; i < 4; ++i) {
              totalCharge += muonCharge[idx[i]];
              p4[i].SetPtEtaPhiM(muonPt[idx[i]], muonEta[idx[i]],
                                 muonPhi[idx[i]], muonMass[idx[i]]);
            }
            if (totalCharge != 0)
              continue;

            bool allOppositeSignMassesPass = true;
            for (int i = 0; i < 4; ++i)
              for (int j = i + 1; j < 4; ++j)
                if (muonCharge[idx[i]] != muonCharge[idx[j]] &&
                    (p4[i] + p4[j]).M() <= 4.)
                  allOppositeSignMassesPass = false;
            if (!allOppositeSignMassesPass)
              continue;

            const int pairings[3][4] = {
                {0, 1, 2, 3}, {0, 2, 1, 3}, {0, 3, 1, 2}};
            for (const auto &pairing : pairings) {
              int i = pairing[0], j = pairing[1], k = pairing[2], l = pairing[3];
              if (muonCharge[idx[i]] == muonCharge[idx[j]] ||
                  muonCharge[idx[k]] == muonCharge[idx[l]])
                continue;
              float firstMass = (p4[i] + p4[j]).M();
              float secondMass = (p4[k] + p4[l]).M();
              if (std::abs(secondMass - 91.1876) < std::abs(firstMass - 91.1876))
                std::swap(firstMass, secondMass);
              if (firstMass < 40. || firstMass > 120. || secondMass < 12. ||
                  secondMass > 120.)
                continue;
              const double score = std::abs(firstMass - 91.1876);
              if (score < bestScore) {
                found = true;
                bestScore = score;
                bestIndices = idx;
                bestZ1 = firstMass;
                bestZ2 = secondMass;
              }
            }
          }

    if (!found)
      continue;

    TLorentzVector candidate;
    for (int i = 0; i < 4; ++i) {
      const auto index = bestIndices[i];
      TLorentzVector muon;
      muon.SetPtEtaPhiM(muonPt[index], muonEta[index], muonPhi[index],
                        muonMass[index]);
      candidate += muon;
      pt[i] = muonPt[index];
      eta[i] = muonEta[index];
      phi[i] = muonPhi[index];
      charge[i] = muonCharge[index];
    }
    outRun = *run;
    outLumi = *luminosityBlock;
    outEvent = *event;
    fourMuonMassValue = candidate.M();
    z1Mass = bestZ1;
    z2Mass = bestZ2;
    massHistogram.Fill(fourMuonMassValue);
    selectedEvents.Fill();
  }

  result.cd();
  TNamed inputMetadata("inputXRootD", input);
  TNamed triggerMetadata("trigger",
                         "HLT_Mu17_TrkIsoVVL_Mu8_TrkIsoVVL_DZ_Mass3p8");
  inputMetadata.Write();
  triggerMetadata.Write();
  massHistogram.Write();
  selectedEvents.Write();

  gStyle->SetOptStat(0);
  TCanvas canvas("canvas", "Four-muon invariant mass", 900, 700);
  canvas.SetLeftMargin(0.13);
  canvas.SetBottomMargin(0.12);
  massHistogram.SetLineColor(kAzure + 2);
  massHistogram.SetFillColorAlpha(kAzure - 9, 0.65);
  massHistogram.SetLineWidth(2);
  massHistogram.Draw("HIST");
  canvas.SaveAs("fourMuonMass.pdf");
  canvas.SaveAs("fourMuonMass.png");
  const auto inputEntries = events->GetEntries();
  const auto selectedEntries = selectedEvents.GetEntries();
  std::cout << "Input events: " << inputEntries << '\n'
            << "Triggered events: " << triggered << '\n'
            << "Selected four-muon candidates: " << selectedEntries << std::endl;
  result.Close();
  source->Close();
}
