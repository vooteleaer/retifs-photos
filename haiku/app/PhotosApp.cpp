/*
 * retifs Photos for Haiku, with the Haiku API: the same album as on Linux.
 * Every photo in the libraries by month, newest first, as square thumbnails
 * (the blocks the extractor keeps with each photo); favourites, albums and
 * people (contacts) to narrow it to; and a viewer with what's known of a
 * photo, favourite, people to tag and albums to put it in.
 *
 * Everything comes from the shared Rust core through retifs_photos.h: JSON
 * read into BMessages with BJson, images as bytes decoded by the Translation
 * Kit. The core's calls block, so they run in a looper of their own (Worker),
 * which answers whoever asked.
 */

#include <Application.h>
#include <Bitmap.h>
#include <Button.h>
#include <CheckBox.h>
#include <DataIO.h>
#include <GroupView.h>
#include <LayoutBuilder.h>
#include <ListItem.h>
#include <ListView.h>
#include <MenuField.h>
#include <MenuItem.h>
#include <MessageRunner.h>
#include <PopUpMenu.h>
#include <ScrollBar.h>
#include <ScrollView.h>
#include <SeparatorView.h>
#include <StringList.h>
#include <StringView.h>
#include <TextControl.h>
#include <TranslationUtils.h>
#include <Window.h>

#include <Json.h>

#include <Screen.h>

#include <algorithm>
#include <deque>
#include <map>
#include <stdlib.h>
#include <string>
#include <vector>

#include "retifs_photos.h"

static const char* kSignature = "application/x-vnd.retifs-photos";
static const float kCell = 150;
static const float kGap = 4;
static const float kHeader = 30;
static const uint64 kMaxThumb = 4 << 20;
static const uint64 kMaxPreview = 32 << 20;

enum {
	kCall = 'call',
	kAnswer = 'answ',
	kImage = 'imag',		// an image the worker fetched and scaled ("id", "role",
							// "bitmap", and for a thumb "square")
	kCheck = 'chck',
	kView = 'view',			// the sidebar's choice changed
	kOpen = 'open',			// the grid: open photo "index"
	kNeed = 'need',			// the grid: the squares shown now and missing ("id"…)
	kChanged = 'chgd',		// the viewer changed something: reload
	kReloaded = 'rldd',		// to the viewer: the load is new, ask again
	kFavorite = 'favr',
	kUntag = 'untg',
	kTag = 'tag_',
	kNewPerson = 'nwpr',
	kAlbum = 'albm',
	kNewAlbum = 'nwal',
	kPrev = 'prev',
	kNext = 'next',
	kText = 'text',			// the text asked for ("text", "for")
	kOk = 'ok__',
	kCancel = 'cncl',
};


// #pragma mark - helpers


static BMessage
Take(char* json, BString* raw = NULL)
{
	BMessage m;
	if (json != NULL) {
		BJson::Parse(json, m);
		if (raw != NULL)
			raw->SetTo(json);
		rp_free(json);
	}
	return m;
}


static int32
Count(const BMessage& array)
{
	return array.HasString("error") ? 0 : array.CountNames(B_ANY_TYPE);
}


static BMessage
Item(const BMessage& array, int32 i)
{
	BMessage item;
	BString name;
	name << i;
	array.FindMessage(name.String(), &item);
	return item;
}


static BString
ItemString(const BMessage& array, int32 i)
{
	BString name;
	name << i;
	return array.GetString(name.String(), "");
}


static bool
Contains(const BMessage& array, const char* value)
{
	for (int32 i = 0; i < Count(array); i++) {
		if (ItemString(array, i) == value)
			return true;
	}
	return false;
}


static BString
MonthName(const BString& ym)
{
	static const char* months[] = { "January", "February", "March", "April",
		"May", "June", "July", "August", "September", "October", "November",
		"December" };
	int m = ym.Length() >= 7 ? atoi(ym.String() + 5) : 0;
	if (m < 1 || m > 12)
		return "No date";
	BString s;
	s << months[m - 1] << " " << BString(ym.String(), 4);
	return s;
}


/*! An image's bytes (JPEG, PNG…) as a bitmap, by the Translation Kit. */
static BBitmap*
Decode(const void* data, size_t length)
{
	if (data == NULL || length == 0)
		return NULL;
	BMemoryIO io(data, length);
	BBitmap* bitmap = BTranslationUtils::GetBitmap(&io);
	if (bitmap == NULL || !bitmap->IsValid()) {
		delete bitmap;
		return NULL;
	}
	if (bitmap->ColorSpace() != B_RGBA32 && bitmap->ColorSpace() != B_RGB32) {
		BBitmap* rgba = new BBitmap(bitmap->Bounds(), B_RGBA32);
		rgba->ImportBits(bitmap);
		delete bitmap;
		bitmap = rgba;
	}
	return bitmap;
}


/*! `source` (or its middle square, if `square`) scaled down to fit
	width×height, each pixel the average of those it covers. */
static BBitmap*
Scaled(const BBitmap* source, int width, int height, bool square)
{
	int sw = source->Bounds().IntegerWidth() + 1;
	int sh = source->Bounds().IntegerHeight() + 1;
	int sx0 = 0, sy0 = 0;
	if (square) {
		int side = std::min(sw, sh);
		sx0 = (sw - side) / 2;
		sy0 = (sh - side) / 2;
		sw = sh = side;
	}
	double scale = std::min(1.0, std::min((double)width / sw, (double)height / sh));
	int dw = std::max(1, (int)(sw * scale)), dh = std::max(1, (int)(sh * scale));
	BBitmap* out = new BBitmap(BRect(0, 0, dw - 1, dh - 1), B_RGBA32);
	const uint8* in = (const uint8*)source->Bits();
	int inRow = source->BytesPerRow();
	bool alpha = source->ColorSpace() == B_RGBA32;
	uint8* to = (uint8*)out->Bits();
	int outRow = out->BytesPerRow();
	for (int y = 0; y < dh; y++) {
		int ya = sy0 + y * sh / dh, yb = std::max(ya + 1, sy0 + (y + 1) * sh / dh);
		for (int x = 0; x < dw; x++) {
			int xa = sx0 + x * sw / dw, xb = std::max(xa + 1, sx0 + (x + 1) * sw / dw);
			uint32 sum[4] = { 0, 0, 0, 0 }, n = 0;
			for (int yy = ya; yy < yb; yy++) {
				const uint8* p = in + yy * inRow + xa * 4;
				for (int xx = xa; xx < xb; xx++, p += 4, n++) {
					sum[0] += p[0];
					sum[1] += p[1];
					sum[2] += p[2];
					sum[3] += alpha ? p[3] : 255;
				}
			}
			uint8* q = to + y * outRow + x * 4;
			for (int c = 0; c < 4; c++)
				q[c] = sum[c] / n;
		}
	}
	return out;
}


/*! The longest side a viewer shows (a preview may be a whole camera image). */
static const int kViewerSide = 2560;
/*! The most squares kept (~90 KB each): those not drawn longest go first,
	a library may have thousands of photos. */
static const size_t kMaxSquares = 1000;


// #pragma mark - Worker


class Worker : public BLooper {
public:
	Worker() : BLooper("photos worker") {}

	void MessageReceived(BMessage* message)
	{
		if (message->what != kCall) {
			BLooper::MessageReceived(message);
			return;
		}
		BString op = message->GetString("op", "");
		const char* a = message->GetString("a", "");
		const char* b = message->GetString("b", "");
		int32 n = message->GetInt32("n", 0);
		BMessenger replyTo;
		message->FindMessenger("reply_to", &replyTo);

		if (op == "thumb" || op == "preview") {
			size_t len = 0;
			uint8_t* bytes = rp_image(a, op == "thumb" ? "thumb" : "preview",
				op == "thumb" ? kMaxThumb : kMaxPreview, &len);
			BMessage image(kImage);
			image.AddString("id", a);
			image.AddString("role", op);
			// Decoded and scaled down here, the whole image dropped at once:
			// a camera's photo is ~48 MB decoded, and app_server's memory for
			// bitmaps ran out with a library's worth of them kept.
			BBitmap* full = Decode(bytes, len);
			if (bytes != NULL)
				rp_free_bytes(bytes, len);
			BBitmap* small = NULL;
			BBitmap* square = NULL;
			if (full != NULL && op == "thumb")
				square = Scaled(full, (int)kCell, (int)kCell, true);
			else if (full != NULL)
				small = Scaled(full, kViewerSide, kViewerSide, false);
			delete full;
			image.AddPointer("bitmap", small);
			image.AddPointer("square", square);
			if (replyTo.SendMessage(&image) != B_OK) {
				delete small;
				delete square;
			}
			return;
		}

		BString raw;
		BMessage result;
		if (op == "load")
			result = Take(rp_load());
		else if (op == "counts")
			result = Take(rp_change_counts(), &raw);
		else if (op == "details")
			result = Take(rp_details(a));
		else if (op == "favorite")
			result = Take(rp_set_favorite(a, n));
		else if (op == "tag")
			result = Take(rp_tag(a, b, n));
		else if (op == "file")
			result = Take(rp_file(a, b, n));
		else if (op == "new_album")
			result = Take(rp_new_album(a, b));
		else if (op == "new_person")
			result = Take(rp_new_person(a, b));

		BMessage answer(kAnswer);
		answer.AddString("op", op);
		answer.AddString("id", a);
		answer.AddMessage("result", &result);
		answer.AddString("raw", raw);
		replyTo.SendMessage(&answer);
	}
};


static Worker* sWorker = NULL;


static void
Call(BHandler* replyTo, const char* op, const char* a = "", const char* b = "",
	int32 n = 0)
{
	BMessage call(kCall);
	call.AddString("op", op);
	call.AddString("a", a);
	call.AddString("b", b);
	call.AddInt32("n", n);
	call.AddMessenger("reply_to", BMessenger(replyTo));
	sWorker->PostMessage(&call);
}


// #pragma mark - TextWindow


/*! Asks for a name: sends "target" a kText with "text" and "for". */
class TextWindow : public BWindow {
public:
	TextWindow(const char* title, const char* body, BMessenger target,
		const char* purpose)
		:
		BWindow(BRect(250, 250, 650, 380), title, B_FLOATING_WINDOW_LOOK,
			B_NORMAL_WINDOW_FEEL, B_AUTO_UPDATE_SIZE_LIMITS | B_CLOSE_ON_ESCAPE),
		fTarget(target),
		fPurpose(purpose)
	{
		fText = new BTextControl("Name", "", NULL);
		BButton* ok = new BButton("Create", new BMessage(kOk));
		BLayoutBuilder::Group<>(this, B_VERTICAL)
			.SetInsets(B_USE_WINDOW_SPACING)
			.Add(new BStringView("body", body))
			.Add(fText)
			.AddGroup(B_HORIZONTAL)
				.AddGlue()
				.Add(new BButton("Cancel", new BMessage(kCancel)))
				.Add(ok)
			.End();
		SetDefaultButton(ok);
		fText->MakeFocus(true);
		CenterOnScreen();
	}

	void MessageReceived(BMessage* message)
	{
		switch (message->what) {
			case kOk:
			{
				BString text(fText->Text());
				text.Trim();
				if (!text.IsEmpty()) {
					BMessage m(kText);
					m.AddString("text", text);
					m.AddString("for", fPurpose);
					fTarget.SendMessage(&m);
				}
				Quit();
				break;
			}
			case kCancel:
				Quit();
				break;
			default:
				BWindow::MessageReceived(message);
		}
	}

private:
	BMessenger fTarget;
	BString fPurpose;
	BTextControl* fText;
};


// #pragma mark - PhotoGrid


struct Shown {
	BString id;
	BString month;
	bool favorite;
};


/*!	The photos of the current view as square thumbnails, by month. */
class PhotoGrid : public BView {
public:
	PhotoGrid(std::map<std::string, BBitmap*>& squares,
		std::map<std::string, uint64>& used)
		:
		BView("grid", B_WILL_DRAW | B_FRAME_EVENTS),
		fSquares(squares),
		fUsed(used),
		fDraws(0)
	{
		SetViewUIColor(B_DOCUMENT_BACKGROUND_COLOR);
	}

	void SetPhotos(const std::vector<Shown>& photos)
	{
		fPhotos = photos;
		_Layout();
		ScrollTo(0, 0);
		Invalidate();
	}

	void FrameResized(float, float)
	{
		_Layout();
		Invalidate();
	}

	void Draw(BRect update)
	{
		SetHighUIColor(B_DOCUMENT_TEXT_COLOR);
		BFont bold(be_bold_font);
		bold.SetSize(bold.Size() * 1.2);
		SetFont(&bold);
		for (size_t i = 0; i < fHeads.size(); i++)
			DrawString(MonthName(fHeads[i].second), fHeads[i].first);
		SetFont(be_plain_font);
		fDraws++;
		BRect visible = Bounds();
		BMessage need(kNeed);
		for (size_t i = 0; i < fPhotos.size(); i++) {
			BRect r = fCells[i];
			std::string id = fPhotos[i].id.String();
			// Shown now, or just below: asked for, nearest first.
			BRect soon = visible;
			soon.bottom += visible.Height();
			if (r.Intersects(soon) && fSquares.count(id) == 0)
				need.AddString("id", id.c_str());
			if (!r.Intersects(update))
				continue;
			auto found = fSquares.find(id);
			if (found != fSquares.end()) {
				fUsed[id] = fDraws;
				DrawBitmap(found->second, r);
			} else {
				SetHighColor(200, 200, 200);
				FillRect(r);
			}
			if (fPhotos[i].favorite) {
				SetHighColor(255, 210, 0);
				DrawString("★", BPoint(r.right - 16, r.top + 16));
			}
		}
		if (need.HasString("id"))
			Window()->PostMessage(&need);
	}

	void MouseDown(BPoint where)
	{
		for (size_t i = 0; i < fCells.size(); i++) {
			if (fCells[i].Contains(where)) {
				BMessage open(kOpen);
				open.AddInt32("index", i);
				Window()->PostMessage(&open);
				return;
			}
		}
	}

	/*! What the scroll view's bar may scroll over. */
	void _Layout()
	{
		fCells.clear();
		fHeads.clear();
		float width = Bounds().Width();
		int32 columns = (int32)((width - kGap) / (kCell + kGap));
		if (columns < 1)
			columns = 1;
		float y = 0;
		int32 column = 0;
		BString month;
		for (size_t i = 0; i < fPhotos.size(); i++) {
			if (i == 0 || fPhotos[i].month != month) {
				if (i > 0)
					y += kCell + kGap;
				month = fPhotos[i].month;
				y += kHeader;
				fHeads.push_back(std::make_pair(BPoint(kGap, y - 8), month));
				column = 0;
			} else if (column == columns) {
				column = 0;
				y += kCell + kGap;
			}
			float x = kGap + column * (kCell + kGap);
			fCells.push_back(BRect(x, y, x + kCell - 1, y + kCell - 1));
			column++;
		}
		float height = fPhotos.empty() ? 0 : y + kCell + kGap;
		BScrollBar* bar = ScrollBar(B_VERTICAL);
		if (bar != NULL) {
			float visible = Bounds().Height();
			bar->SetRange(0, height > visible ? height - visible : 0);
			bar->SetProportion(height > 0 ? visible / height : 1);
			bar->SetSteps(kCell / 4, visible);
		}
	}

	void Refresh() { Invalidate(); }

private:
	std::map<std::string, BBitmap*>& fSquares;
	std::map<std::string, uint64>& fUsed;
	uint64 fDraws;
	std::vector<Shown> fPhotos;
	std::vector<BRect> fCells;
	std::vector<std::pair<BPoint, BString>> fHeads;
};


// #pragma mark - ImageView


/*! A picture, scaled to fit. */
class ImageView : public BView {
public:
	ImageView() : BView("image", B_WILL_DRAW | B_FRAME_EVENTS), fBitmap(NULL)
	{
		SetViewColor(30, 30, 30);
		SetExplicitMinSize(BSize(420, 360));
	}

	~ImageView() { delete fBitmap; }

	void SetBitmap(BBitmap* bitmap)
	{
		delete fBitmap;
		fBitmap = bitmap;
		Invalidate();
	}

	void FrameResized(float, float) { Invalidate(); }

	void Draw(BRect)
	{
		if (fBitmap == NULL)
			return;
		BRect b = Bounds(), s = fBitmap->Bounds();
		float scale = std::min(b.Width() / s.Width(), b.Height() / s.Height());
		float w = s.Width() * scale, h = s.Height() * scale;
		BRect to(0, 0, w, h);
		to.OffsetTo((b.Width() - w) / 2, (b.Height() - h) / 2);
		DrawBitmap(fBitmap, s, to, B_FILTER_BITMAP_BILINEAR);
	}

private:
	BBitmap* fBitmap;
};


// #pragma mark - ViewerWindow


class PhotosWindow;


/*! Puts `window` over the whole screen, its tab and border just outside. */
static void
FillScreen(BWindow* window)
{
	BRect screen = BScreen(window).Frame();
	BRect frame = window->Frame(), outer = window->DecoratorFrame();
	float left = frame.left - outer.left, top = frame.top - outer.top;
	float right = outer.right - frame.right, bottom = outer.bottom - frame.bottom;
	window->MoveTo(screen.left + left, screen.top + top);
	window->ResizeTo(screen.Width() - left - right, screen.Height() - top - bottom);
}


/*! One photo: its picture, what's known of it, favourite, people, albums. */
class ViewerWindow : public BWindow {
public:
	ViewerWindow(BMessenger main, const BMessage& load, const BString& id,
		bool hasPrev, bool hasNext, BBitmap* thumb)
		:
		BWindow(BRect(120, 80, 1020, 680), "Photo", B_TITLED_WINDOW,
			B_AUTO_UPDATE_SIZE_LIMITS | B_CLOSE_ON_ESCAPE),
		fMain(main),
		fLoad(load),
		fId(id)
	{
		fImage = new ImageView();
		if (thumb != NULL)
			fImage->SetBitmap(new BBitmap(thumb));
		FillScreen(this);
		fTitle = new BStringView("title", "");
		BFont bold(be_bold_font);
		bold.SetSize(bold.Size() * 1.2);
		fTitle->SetFont(&bold);
		fFavorite = new BCheckBox("Favorite", new BMessage(kFavorite));
		fDetails = new BGroupView(B_VERTICAL, 2);
		fTagged = new BListView("tagged");
		fTag = new BMenuField("Add", new BPopUpMenu("Tag a person" B_UTF8_ELLIPSIS));
		fAlbums = new BGroupView(B_VERTICAL, 0);
		BButton* prev = new BButton("◀", new BMessage(kPrev));
		BButton* next = new BButton("▶", new BMessage(kNext));
		prev->SetEnabled(hasPrev);
		next->SetEnabled(hasNext);

		BLayoutBuilder::Group<>(this, B_HORIZONTAL)
			.SetInsets(B_USE_WINDOW_SPACING)
			.Add(fImage, 3)
			.AddGroup(B_VERTICAL, B_USE_DEFAULT_SPACING, 1)
				.AddGroup(B_HORIZONTAL)
					.Add(fTitle)
					.AddGlue()
					.Add(prev)
					.Add(next)
				.End()
				.Add(fFavorite)
				.Add(fDetails)
				.Add(new BSeparatorView(B_HORIZONTAL))
				.Add(new BStringView("p", "People (your contacts)"))
				.Add(new BScrollView("ts", fTagged, 0, false, true))
				.AddGroup(B_HORIZONTAL)
					.Add(fTag)
					.Add(new BButton("Untag", new BMessage(kUntag)))
				.End()
				.Add(new BSeparatorView(B_HORIZONTAL))
				.Add(new BStringView("a", "Albums"))
				.Add(fAlbums)
				.Add(new BButton("New album with this photo",
					new BMessage(kNewAlbum)))
				.AddGlue()
			.End();

		Call(this, "details", fId.String());
		Call(this, "preview", fId.String());
	}

	void MessageReceived(BMessage* message)
	{
		switch (message->what) {
			case kAnswer:
				_Answer(message);
				break;
			case kImage:
			{
				BBitmap* bitmap = NULL;
				BBitmap* square = NULL;
				message->FindPointer("bitmap", (void**)&bitmap);
				message->FindPointer("square", (void**)&square);
				delete square;
				if (bitmap != NULL)
					fImage->SetBitmap(bitmap);
				break;
			}
			case kReloaded:
				message->FindMessage("load", &fLoad);
				Call(this, "details", fId.String());
				break;
			case kFavorite:
				Call(this, "favorite", fId.String(), "",
					fFavorite->Value() == B_CONTROL_ON);
				break;
			case kTag:
				Call(this, "tag", fId.String(), message->GetString("person", ""), 1);
				break;
			case kUntag:
			{
				int32 i = fTagged->CurrentSelection();
				if (i >= 0)
					Call(this, "tag", fId.String(),
						fTaggedIds.StringAt(i).String(), 0);
				break;
			}
			case kNewPerson:
				(new TextWindow("New contact", "They're added to your address "
					"book (me) and tagged here.", BMessenger(this), "person"))->Show();
				break;
			case kAlbum:
			{
				BString album = message->GetString("album", "");
				BCheckBox* box = dynamic_cast<BCheckBox*>(
					fAlbums->FindView(album.String()));
				if (box != NULL)
					Call(this, "file", fId.String(), album.String(),
						box->Value() == B_CONTROL_ON);
				break;
			}
			case kNewAlbum:
				(new TextWindow("New album", "Albums are records in a library: "
					"share one and its photos go with it.", BMessenger(this),
					"album"))->Show();
				break;
			case kText:
			{
				BString text = message->GetString("text", "");
				if (BString(message->GetString("for", "")) == "album")
					Call(this, "new_album", text.String(), fId.String());
				else
					Call(this, "new_person", text.String(), fId.String());
				break;
			}
			case kPrev:
			case kNext:
			{
				BMessage step(message->what);
				step.AddString("id", fId);
				fMain.SendMessage(&step);
				Quit();
				break;
			}
			default:
				BWindow::MessageReceived(message);
		}
	}

private:
	void _Answer(BMessage* message)
	{
		BString op = message->GetString("op", "");
		BMessage r;
		message->FindMessage("result", &r);
		if (op == "details") {
			_Show(r);
			return;
		}
		// A change: the window reloads, then tells this one to ask again.
		fMain.SendMessage(kChanged);
	}

	void _Show(const BMessage& d)
	{
		fTitle->SetText(d.GetString("title", ""));
		SetTitle(d.GetString("title", "Photo"));
		fFavorite->SetValue(d.GetBool("favorite", false) ? B_CONTROL_ON
			: B_CONTROL_OFF);

		while (BView* child = fDetails->ChildAt(0)) {
			child->RemoveSelf();
			delete child;
		}
		BMessage details;
		d.FindMessage("details", &details);
		for (int32 i = 0; i < Count(details); i++) {
			BMessage pair = Item(details, i);
			BString line;
			line << ItemString(pair, 0) << ": " << ItemString(pair, 1);
			fDetails->AddChild(new BStringView("d", line));
		}

		// People: those tagged, and the others to tag (or a new contact).
		BMessage people, everyone;
		d.FindMessage("people", &people);
		fLoad.FindMessage("people", &everyone);
		fTagged->MakeEmpty();
		fTaggedIds.MakeEmpty();
		for (int32 i = 0; i < Count(people); i++) {
			BMessage pair = Item(people, i);
			fTaggedIds.Add(ItemString(pair, 0));
			fTagged->AddItem(new BStringItem(ItemString(pair, 1)));
		}
		BMenu* menu = fTag->Menu();
		while (BMenuItem* item = menu->RemoveItem((int32)0))
			delete item;
		for (int32 i = 0; i < Count(everyone); i++) {
			BMessage p = Item(everyone, i);
			BString id = p.GetString("id", "");
			if (fTaggedIds.HasString(id))
				continue;
			BMessage* tag = new BMessage(kTag);
			tag->AddString("person", id);
			menu->AddItem(new BMenuItem(p.GetString("name", ""), tag));
		}
		menu->AddSeparatorItem();
		menu->AddItem(new BMenuItem("New contact" B_UTF8_ELLIPSIS,
			new BMessage(kNewPerson)));
		menu->SetTargetForItems(this);

		// Albums: in or out of each.
		while (BView* child = fAlbums->ChildAt(0)) {
			child->RemoveSelf();
			delete child;
		}
		BMessage in, albums;
		d.FindMessage("albums", &in);
		fLoad.FindMessage("albums", &albums);
		for (int32 i = 0; i < Count(albums); i++) {
			BMessage a = Item(albums, i);
			BString id = a.GetString("id", "");
			BMessage* toggle = new BMessage(kAlbum);
			toggle->AddString("album", id);
			BCheckBox* box = new BCheckBox(id.String(), a.GetString("name", ""),
				toggle);
			box->SetValue(Contains(in, id.String()) ? B_CONTROL_ON : B_CONTROL_OFF);
			fAlbums->AddChild(box);
			box->SetTarget(this);
		}
	}

	BMessenger fMain;
	BMessage fLoad;
	BString fId;
	ImageView* fImage;
	BStringView* fTitle;
	BCheckBox* fFavorite;
	BGroupView* fDetails;
	BListView* fTagged;
	BStringList fTaggedIds;
	BMenuField* fTag;
	BGroupView* fAlbums;
};


// #pragma mark - PhotosWindow


class PhotosWindow : public BWindow {
public:
	PhotosWindow()
		:
		BWindow(BRect(60, 60, 1060, 720), "Photos", B_TITLED_WINDOW,
			B_AUTO_UPDATE_SIZE_LIMITS | B_QUIT_ON_WINDOW_CLOSE),
		fViewKind("all")
	{
		fSide = new BListView("side");
		fSide->SetSelectionMessage(new BMessage(kView));
		fTitle = new BStringView("title", "Photos");
		BFont big(be_bold_font);
		big.SetSize(big.Size() * 1.3);
		fTitle->SetFont(&big);
		fStatus = new BStringView("status", "");
		fGrid = new PhotoGrid(fSquares, fUsed);

		BScrollView* side = new BScrollView("ss", fSide, 0, false, true);
		side->SetExplicitPreferredSize(BSize(240, B_SIZE_UNSET));
		BLayoutBuilder::Group<>(this, B_HORIZONTAL)
			.SetInsets(B_USE_WINDOW_SPACING)
			.Add(side, 1)
			.AddGroup(B_VERTICAL, B_USE_DEFAULT_SPACING, 4)
				.AddGroup(B_HORIZONTAL)
					.Add(fTitle)
					.AddGlue()
					.Add(fStatus)
				.End()
				.Add(new BScrollView("gs", fGrid, 0, false, true))
			.End();

		Call(this, "load");
		fRunner = new BMessageRunner(BMessenger(this), BMessage(kCheck), 3000000);
	}

	~PhotosWindow()
	{
		delete fRunner;
		for (auto& s : fSquares)
			delete s.second;
	}

	void MessageReceived(BMessage* message)
	{
		switch (message->what) {
			case kAnswer:
			{
				BString op = message->GetString("op", "");
				BMessage r;
				message->FindMessage("result", &r);
				if (op == "load") {
					fLoad = r;
					_ShowSidebar();
					_ShowGrid();
					BMessage reloaded(kReloaded);
					reloaded.AddMessage("load", &fLoad);
					fViewer.SendMessage(&reloaded);
				} else if (op == "counts") {
					BString raw = message->GetString("raw", "");
					if (!fCounts.IsEmpty() && raw != fCounts)
						Call(this, "load");
					fCounts = raw;
				}
				break;
			}
			case kImage:
			{
				BString id = message->GetString("id", "");
				BBitmap* small = NULL;
				BBitmap* square = NULL;
				message->FindPointer("bitmap", (void**)&small);
				message->FindPointer("square", (void**)&square);
				delete small;
				fAsked.erase(id.String());
				fInFlight = std::max(0, fInFlight - 1);
				if (square != NULL) {
					delete fSquares[id.String()];
					fSquares[id.String()] = square;
					_Trim();
					fGrid->Refresh();
				}
				_Pump();
				break;
			}
			case kNeed:
			{
				// What's shown now replaces what was wanted before.
				fWanted.clear();
				const char* id;
				for (int32 i = 0; message->FindString("id", i, &id) == B_OK; i++)
					fWanted.push_back(id);
				_Pump();
				break;
			}
			case kCheck:
				Call(this, "counts");
				break;
			case kView:
			{
				int32 i = fSide->CurrentSelection();
				if (i < 0 || i >= (int32)fSideViews.size()
					|| fSideViews[i].first.IsEmpty())
					break;
				fViewKind = fSideViews[i].first;
				fViewId = fSideViews[i].second;
				_ShowGrid();
				break;
			}
			case kOpen:
				_Open(message->GetInt32("index", 0));
				break;
			case kPrev:
			case kNext:
			{
				BString id = message->GetString("id", "");
				for (size_t i = 0; i < fShown.size(); i++) {
					if (fShown[i].id == id) {
						_Open(message->what == kPrev ? i - 1 : i + 1);
						break;
					}
				}
				break;
			}
			case kChanged:
				Call(this, "load");
				break;
			default:
				BWindow::MessageReceived(message);
		}
	}

private:
	void _ShowSidebar()
	{
		BMessage photos, albums, people;
		fLoad.FindMessage("photos", &photos);
		fLoad.FindMessage("albums", &albums);
		fLoad.FindMessage("people", &people);
		int32 favorites = 0;
		for (int32 i = 0; i < Count(photos); i++)
			favorites += Item(photos, i).GetBool("favorite", false);

		int32 selected = 0;
		fSide->MakeEmpty();
		fSideViews.clear();
		auto add = [&](const BString& text, const char* kind, const BString& id) {
			BStringItem* item = new BStringItem(text);
			if (*kind == '\0')
				item->SetEnabled(false);
			fSide->AddItem(item);
			if (fViewKind == kind && fViewId == id)
				selected = fSide->CountItems() - 1;
			fSideViews.push_back(std::make_pair(BString(kind), id));
		};
		BString line;
		line << "Photos  ·  " << Count(photos);
		add(line, "all", "");
		line = "Favorites  ·  ";
		line << favorites;
		add(line, "favorites", "");
		add("Albums", "", "");
		for (int32 i = 0; i < Count(albums); i++) {
			BMessage a = Item(albums, i);
			BString id = a.GetString("id", "");
			int32 n = 0;
			for (int32 j = 0; j < Count(photos); j++) {
				BMessage in;
				Item(photos, j).FindMessage("albums", &in);
				n += Contains(in, id.String());
			}
			line = "    ";
			line << a.GetString("name", "") << "  ·  " << n;
			add(line, "album", id);
		}
		add("People", "", "");
		for (int32 i = 0; i < Count(people); i++) {
			BMessage p = Item(people, i);
			BString id = p.GetString("id", "");
			int32 n = 0;
			for (int32 j = 0; j < Count(photos); j++) {
				BMessage in;
				Item(photos, j).FindMessage("people", &in);
				n += Contains(in, id.String());
			}
			// People without photos stay in Contacts.
			if (n == 0 && !p.HasString("photo"))
				continue;
			line = "    ";
			line << p.GetString("name", "") << "  ·  " << n;
			add(line, "person", id);
		}
		fSide->Select(selected);
	}

	void _ShowGrid()
	{
		BMessage photos, albums, people;
		fLoad.FindMessage("photos", &photos);
		fShown.clear();
		std::vector<Shown> shown;
		for (int32 i = 0; i < Count(photos); i++) {
			BMessage p = Item(photos, i);
			BMessage in;
			bool show = fViewKind == "all"
				|| (fViewKind == "favorites" && p.GetBool("favorite", false));
			if (fViewKind == "album" && p.FindMessage("albums", &in) == B_OK)
				show = Contains(in, fViewId.String());
			if (fViewKind == "person" && p.FindMessage("people", &in) == B_OK)
				show = Contains(in, fViewId.String());
			if (!show)
				continue;
			Shown s;
			s.id = p.GetString("id", "");
			s.month = p.GetString("month", "");
			s.favorite = p.GetBool("favorite", false);
			shown.push_back(s);
		}
		fShown = shown;
		BString title = fViewKind == "favorites" ? "Favorites" : "Photos";
		for (size_t i = 0; i < fSideViews.size(); i++) {
			if (fSideViews[i].first == fViewKind && fSideViews[i].second == fViewId
				&& !fViewId.IsEmpty()) {
				BStringItem* item = (BStringItem*)fSide->ItemAt(i);
				title = item->Text();
				title.Trim();
				int32 dot = title.FindLast("  ·");
				if (dot > 0)
					title.Truncate(dot);
			}
		}
		fTitle->SetText(title);
		BString status;
		if (fLoad.HasString("error"))
			status << "retifsd isn't answering: " << fLoad.GetString("error", "");
		else
			status << shown.size() << " photo(s)";
		fStatus->SetText(status);
		fGrid->SetPhotos(shown);

		// The thumbnails: asked for as the grid shows them (kNeed).
	}

	/*! Asks for the wanted squares, two at a time (each a fetch, maybe from
		another node): scrolled past, they aren't asked for. */
	void _Pump()
	{
		while (fInFlight < 2 && !fWanted.empty()) {
			std::string id = fWanted.front();
			fWanted.pop_front();
			if (fSquares.count(id) != 0 || fAsked.count(id) != 0)
				continue;
			fAsked[id] = true;
			fInFlight++;
			Call(this, "thumb", id.c_str());
		}
	}

	/*! Keeps at most kMaxSquares, dropping those drawn longest ago. */
	void _Trim()
	{
		while (fSquares.size() > kMaxSquares) {
			auto oldest = fSquares.begin();
			uint64 when = UINT64_MAX;
			for (auto it = fSquares.begin(); it != fSquares.end(); ++it) {
				uint64 used = fUsed.count(it->first) ? fUsed[it->first] : 0;
				if (used < when) {
					when = used;
					oldest = it;
				}
			}
			fUsed.erase(oldest->first);
			delete oldest->second;
			fSquares.erase(oldest);
		}
	}

	void _Open(int32 i)
	{
		if (i < 0 || i >= (int32)fShown.size())
			return;
		auto thumb = fSquares.find(fShown[i].id.String());
		ViewerWindow* viewer = new ViewerWindow(BMessenger(this), fLoad,
			fShown[i].id, i > 0, i + 1 < (int32)fShown.size(),
			thumb != fSquares.end() ? thumb->second : NULL);
		fViewer = BMessenger(viewer);
		viewer->Show();
	}

	BListView* fSide;
	std::vector<std::pair<BString, BString>> fSideViews;
	BStringView* fTitle;
	BStringView* fStatus;
	PhotoGrid* fGrid;
	std::map<std::string, BBitmap*> fSquares;
	std::map<std::string, uint64> fUsed;
	std::deque<std::string> fWanted;
	int32 fInFlight = 0;
	std::map<std::string, bool> fAsked;
	std::vector<Shown> fShown;
	BMessage fLoad;
	BString fViewKind;
	BString fViewId;
	BString fCounts;
	BMessenger fViewer;
	BMessageRunner* fRunner;
};


// #pragma mark - App


class PhotosApp : public BApplication {
public:
	PhotosApp() : BApplication(kSignature) {}

	void ReadyToRun()
	{
		sWorker = new Worker();
		sWorker->Run();
		(new PhotosWindow())->Show();
	}

	bool QuitRequested()
	{
		if (sWorker != NULL && sWorker->Lock())
			sWorker->Quit();
		sWorker = NULL;
		return true;
	}
};


int
main()
{
	PhotosApp app;
	app.Run();
	return 0;
}
