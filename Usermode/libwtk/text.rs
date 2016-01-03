//
//
//
use surface::Colour;
use geom::Rect;

pub struct Label<'a>
{
	colour: Colour,
	value: &'a str,
}

impl<'a> Label<'a>
{
	pub fn new(s: &'a str, colour: Colour) -> Label<'a> {
		Label {
			colour: colour,
			value: s,
		}
	}
}

impl<'a> ::Element for Label<'a>
{
	fn render(&self, surface: ::surface::SurfaceView, force: bool) {
		if force {
			surface.draw_text( Rect::new(0,0,!0,!0), self.value.chars(), self.colour );
		}
	}
	fn element_at_pos(&self, _x: u32, _y: u32) -> (&::Element,(u32,u32)) {
		(self, (0,0))
	}

}

